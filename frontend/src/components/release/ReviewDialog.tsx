import React, { useEffect } from 'react';
import { Bot, ExternalLink, PartyPopper, Rocket, ShieldCheck, X } from 'lucide-react';
import type { ReleaseCheck, ReleaseOutcome, ReleaseReport } from '../../types';
import { Badge, CheckList, Spinner, StatusIcon, TONE, btnGhost, btnPrimary, tint } from './ui';

/**
 * Modal that walks one release through plan → running → result, so it is always clear whether
 * anything has changed yet. A plan is a dry run (nothing changed); a result is real.
 */
export function ReviewDialog({
  plan,
  outcome,
  verify,
  busy,
  modeLabel,
  fromAgent,
  onConfirm,
  onVerify,
  onOpenUrl,
  onClose,
  scale = 1,
}: {
  scale?: number;
  plan: ReleaseOutcome | null;
  outcome: ReleaseOutcome | null;
  verify: ReleaseReport | null;
  busy: string | null;
  modeLabel: string;
  /** The plan came from an agent's dry run (shown in the activity feed). */
  fromAgent?: boolean;
  onConfirm: () => void;
  onVerify: () => void;
  onOpenUrl: (url: string) => void;
  onClose: () => void;
}) {
  const releasing = busy === 'release';
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !releasing) onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose, releasing]);

  const shown = outcome ?? plan;
  if (!shown) return null;
  const blockers = shown.preflight.filter((c) => c.status === 'fail' || c.status === 'warn');
  const failed = blockers.filter((c) => c.status === 'fail').length;

  let phase: 'plan' | 'running' | 'success' | 'failed';
  if (releasing) phase = 'running';
  else if (outcome) phase = outcome.ok ? 'success' : 'failed';
  else phase = 'plan';

  const title =
    phase === 'running'
      ? `Releasing ${shown.tag}…`
      : phase === 'success'
      ? `${shown.tag} is released`
      : phase === 'failed'
      ? `Release of ${shown.tag} stopped`
      : `Release ${shown.tag}?`;

  const subtitle =
    phase === 'running'
      ? 'Running checks, bumping versions, committing, tagging and pushing. Builds and tests can take a minute.'
      : phase === 'success'
      ? 'Everything below has been done.'
      : phase === 'failed'
      ? 'A step failed. See which one below; earlier steps may have completed.'
      : shown.ok
      ? 'This is a dry run. Nothing has changed yet. Confirm to run these steps for real.'
      : `This release is blocked by ${failed} failed check${failed === 1 ? '' : 's'}. Nothing has changed.`;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-[2px] p-4"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && !releasing) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className="w-full max-w-[560px] flex flex-col rounded-xl border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl overflow-hidden"
                style={{ zoom: scale, maxHeight: `${86 / scale}vh` }}
      >
        <header
          className="px-5 pt-5 pb-4 border-b border-[var(--border)]"
          style={
            phase === 'success'
              ? { background: `linear-gradient(180deg, ${tint('ok', 14)}, transparent)` }
              : phase === 'failed'
              ? { background: `linear-gradient(180deg, ${tint('fail', 12)}, transparent)` }
              : undefined
          }
        >
          <div className="flex items-start gap-3">
            <span
              className="w-9 h-9 rounded-lg flex items-center justify-center shrink-0"
              style={{
                background: tint(phase === 'success' ? 'ok' : phase === 'failed' ? 'fail' : 'accent', 18),
                color: TONE[phase === 'success' ? 'ok' : phase === 'failed' ? 'fail' : 'accent'],
              }}
            >
              {phase === 'running' ? <Spinner size={17} /> : phase === 'success' ? <PartyPopper size={17} /> : <Rocket size={17} />}
            </span>
            <div className="min-w-0 flex-1">
              <h2 className="text-[15px] font-semibold text-[var(--text-primary)]">{title}</h2>
              <p className="mt-0.5 text-[12.5px] text-[var(--text-muted)] leading-snug">{subtitle}</p>
              <div className="mt-2 flex flex-wrap gap-1.5">
                <Badge tone="accent">{modeLabel}</Badge>
                {phase === 'plan' && <Badge tone={shown.ok ? 'neutral' : 'fail'}>{shown.ok ? 'dry run' : 'blocked'}</Badge>}
                {fromAgent && (
                  <Badge>
                    <Bot size={11} /> planned by agent
                  </Badge>
                )}
              </div>
            </div>
            <button
              className="p-1 rounded text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer disabled:opacity-30"
              onClick={onClose}
              disabled={releasing}
              aria-label="Close"
            >
              <X size={15} />
            </button>
          </div>
        </header>

        <div className="flex-1 min-h-0 overflow-y-auto px-5 py-4 flex flex-col gap-4 select-text">
          {blockers.length > 0 && (
            <Block title={failed > 0 ? 'Fix these first' : 'Heads up'}>
              <CheckList checks={blockers} dense />
            </Block>
          )}
          <Block title={phase === 'plan' ? 'What will happen' : 'Steps'}>
            {phase === 'running' ? (
              <RunningSteps steps={shown.steps} />
            ) : shown.steps.length > 0 ? (
              <ol className="flex flex-col gap-2">
                {shown.steps.map((c, i) => (
                  <li key={c.id + i} className="flex gap-2.5 text-[12.5px] leading-snug">
                    {phase === 'plan' ? (
                      <span className="w-[18px] h-[18px] mt-px rounded-full shrink-0 flex items-center justify-center text-[10.5px] font-semibold bg-[var(--bg-active)] text-[var(--text-muted)]">
                        {i + 1}
                      </span>
                    ) : (
                      <span className="mt-px">
                        <StatusIcon status={c.status} />
                      </span>
                    )}
                    <span className="min-w-0">
                      <span className="text-[var(--text-primary)]">{c.label}</span>
                      {c.detail && <span className="block text-[11.5px] text-[var(--text-muted)] break-words">{c.detail}</span>}
                    </span>
                  </li>
                ))}
              </ol>
            ) : (
              <p className="text-[12px] text-[var(--text-muted)]">Nothing runs until the checks above pass.</p>
            )}
          </Block>
          {verify && (
            <Block title="Verification">
              <CheckList checks={verify.checks} dense />
            </Block>
          )}
        </div>

        <footer className="px-5 py-3.5 border-t border-[var(--border)] flex items-center gap-2 bg-[var(--bg-base)]/40">
          {phase === 'plan' && (
            <>
              <span className="text-[11.5px] text-[var(--text-dim)] mr-auto">Esc to cancel</span>
              <button className={btnGhost} onClick={onClose}>
                Cancel
              </button>
              <button className={btnPrimary} disabled={!shown.ok || !!busy} onClick={onConfirm}>
                <Rocket size={13} /> Confirm &amp; release {shown.tag}
              </button>
            </>
          )}
          {phase === 'running' && (
            <span className="flex items-center gap-2 text-[12px] text-[var(--text-muted)]">
              <Spinner /> Working. Keep Vireo open until this finishes.
            </span>
          )}
          {(phase === 'success' || phase === 'failed') && (
            <>
              {outcome?.url && (
                <button className={btnGhost} onClick={() => onOpenUrl(outcome.url!)}>
                  <ExternalLink size={13} /> Open on GitHub
                </button>
              )}
              <button className={btnGhost} disabled={!!busy} onClick={onVerify}>
                {busy === 'verify' ? <Spinner /> : <ShieldCheck size={13} />} Verify
              </button>
              <button className={`${btnPrimary} ml-auto`} onClick={onClose}>
                Done
              </button>
            </>
          )}
        </footer>
      </div>
    </div>
  );
}

function Block({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div>
      <h3 className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-[var(--text-dim)]">{title}</h3>
      {children}
    </div>
  );
}

/** While the real run is in flight only the plan is known, so steps show as pending (no fake progress). */
function RunningSteps({ steps }: { steps: ReleaseCheck[] }) {
  return (
    <ol className="flex flex-col gap-2">
      {steps.map((c, i) => (
        <li key={c.id + i} className="flex gap-2.5 text-[12.5px] leading-snug text-[var(--text-muted)]">
          <span className="mt-px">
            <StatusIcon status="skip" />
          </span>
          {c.label}
        </li>
      ))}
    </ol>
  );
}
