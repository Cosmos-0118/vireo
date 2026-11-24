import React from 'react';
import { AlertTriangle, Check, CheckCircle2, CircleDashed, Loader2, XCircle } from 'lucide-react';
import type { ReleaseCheck } from '../../types';

export type Tone = 'neutral' | 'accent' | 'ok' | 'warn' | 'fail';

export const TONE: Record<Tone, string> = {
  neutral: 'var(--text-muted)',
  accent: 'var(--accent)',
  ok: 'var(--added-fg)',
  warn: 'var(--modified-fg)',
  fail: 'var(--removed-fg)',
};

export const tint = (tone: Tone, pct = 14) => `color-mix(in srgb, ${TONE[tone]} ${pct}%, transparent)`;

const btnBase =
  'inline-flex items-center justify-center gap-1.5 rounded-md px-3 h-8 text-[12.5px] font-medium cursor-pointer transition-colors disabled:opacity-40 disabled:cursor-default whitespace-nowrap';
export const btnGhost = `${btnBase} border border-[var(--border)] bg-[var(--bg-base)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)]`;
export const btnPrimary = `${btnBase} bg-[var(--accent)] text-[var(--accent-fg)] hover:bg-[var(--accent-hover)] shadow-sm`;
export const btnQuiet = `${btnBase} px-2 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]`;

export function Spinner({ size = 13 }: { size?: number }) {
  return <Loader2 size={size} className="animate-spin" />;
}

export function Badge({ tone = 'neutral', children, title }: { tone?: Tone; children: React.ReactNode; title?: string }) {
  return (
    <span
      title={title}
      className="inline-flex items-center gap-1 rounded-full px-2 py-px text-[11px] font-medium leading-[18px] whitespace-nowrap"
      style={{ color: TONE[tone], background: tint(tone), border: `1px solid ${tint(tone, 28)}` }}
    >
      {children}
    </span>
  );
}

export function StatusIcon({ status, size = 15 }: { status: ReleaseCheck['status'] | 'running'; size?: number }) {
  const cls = 'shrink-0';
  if (status === 'running') return <Loader2 size={size} className={`${cls} animate-spin`} style={{ color: TONE.accent }} />;
  if (status === 'ok') return <CheckCircle2 size={size} className={cls} style={{ color: TONE.ok }} />;
  if (status === 'fail') return <XCircle size={size} className={cls} style={{ color: TONE.fail }} />;
  if (status === 'warn') return <AlertTriangle size={size} className={cls} style={{ color: TONE.warn }} />;
  return <CircleDashed size={size} className={cls} style={{ color: 'var(--text-dim)' }} />;
}

export function CheckList({ checks, dense }: { checks: ReleaseCheck[]; dense?: boolean }) {
  return (
    <ul className="flex flex-col divide-y divide-[var(--border)]">
      {checks.map((c, i) => (
        <li key={c.id + i} className={`flex gap-2.5 ${dense ? 'py-1.5' : 'py-2'} first:pt-0 last:pb-0 text-[12.5px] leading-snug`}>
          <span className="mt-px">
            <StatusIcon status={c.status} />
          </span>
          <span className="min-w-0">
            <span className="text-[var(--text-primary)]">{c.label}</span>
            {c.detail && <span className="block text-[11.5px] text-[var(--text-muted)] break-words">{c.detail}</span>}
          </span>
        </li>
      ))}
    </ul>
  );
}

export function Toggle({
  checked,
  onChange,
  label,
  hint,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  hint?: string;
  disabled?: boolean;
}) {
  return (
    <label
      className={`flex items-start gap-2.5 text-[12.5px] select-none ${disabled ? 'opacity-50' : 'cursor-pointer'}`}
      onClick={(e) => {
        e.preventDefault();
        if (!disabled) onChange(!checked);
      }}
    >
      <span
        role="switch"
        aria-checked={checked}
        className="relative mt-px w-8 h-[18px] rounded-full shrink-0 transition-colors"
        style={{ background: checked ? 'var(--accent)' : 'var(--bg-active)' }}
      >
        <span
          className="absolute top-[3px] left-[3px] w-3 h-3 rounded-full bg-white transition-transform"
          style={{ transform: checked ? 'translateX(14px)' : 'none' }}
        />
      </span>
      <span className="min-w-0">
        <span className="text-[var(--text-primary)]">{label}</span>
        {hint && <span className="block text-[11.5px] text-[var(--text-muted)] leading-snug">{hint}</span>}
      </span>
    </label>
  );
}

export type StepState = 'todo' | 'done' | 'warn' | 'fail' | 'running';

/** A numbered step on the release page: a status dot on a vertical rail, then a card. */
export function Step({
  n,
  id,
  title,
  subtitle,
  state,
  badge,
  actions,
  last,
  children,
}: {
  n: number;
  id: string;
  title: string;
  subtitle?: React.ReactNode;
  state: StepState;
  badge?: React.ReactNode;
  actions?: React.ReactNode;
  last?: boolean;
  children: React.ReactNode;
}) {
  const tone: Tone = state === 'done' ? 'ok' : state === 'warn' ? 'warn' : state === 'fail' ? 'fail' : 'accent';
  return (
    <section id={id} className="relative flex gap-4 scroll-mt-4">
      <div className="flex flex-col items-center shrink-0">
        <span
          className="flex items-center justify-center w-7 h-7 rounded-full text-[12px] font-semibold"
          style={
            state === 'done'
              ? { background: TONE.ok, color: 'var(--bg-base)' }
              : state === 'todo' || state === 'running'
              ? { background: tint('accent', 14), color: TONE.accent, border: `1px solid ${tint('accent', 40)}` }
              : { background: tint(tone, 18), color: TONE[tone], border: `1px solid ${tint(tone, 45)}` }
          }
        >
          {state === 'done' ? (
            <Check size={14} strokeWidth={3} />
          ) : state === 'running' ? (
            <Spinner size={13} />
          ) : state === 'fail' ? (
            <XCircle size={14} />
          ) : state === 'warn' ? (
            <AlertTriangle size={13} />
          ) : (
            n
          )}
        </span>
        {!last && <span className="flex-1 w-px mt-1.5 bg-[var(--border)]" />}
      </div>
      <div className={`min-w-0 flex-1 ${last ? '' : 'pb-6'}`}>
        <header className="flex items-start gap-3 min-h-7">
          <div className="min-w-0 flex-1">
            <h2 className="flex items-center gap-2 text-[14px] font-semibold text-[var(--text-primary)] leading-7">
              {title}
              {badge}
            </h2>
            {subtitle && <p className="text-[12px] text-[var(--text-muted)] leading-snug -mt-0.5">{subtitle}</p>}
          </div>
          {actions && <div className="flex items-center gap-1.5 shrink-0 pt-0.5">{actions}</div>}
        </header>
        <div className="mt-3">{children}</div>
      </div>
    </section>
  );
}

export function Panel({ children, className = '' }: { children: React.ReactNode; className?: string }) {
  return (
    <div className={`rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] ${className}`}>{children}</div>
  );
}

export function relTime(ms: number): string {
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 10) return 'just now';
  if (s < 60) return `${s}s ago`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.round(h / 24);
  if (d < 30) return `${d}d ago`;
  return new Date(ms).toLocaleDateString();
}

export const splitPath = (p: string) => {
  const i = p.lastIndexOf('/');
  return i < 0 ? { dir: '', name: p } : { dir: p.slice(0, i), name: p.slice(i + 1) };
};

export const isLock = (p: string) => /lock\.json$|Cargo\.lock$/.test(p);
