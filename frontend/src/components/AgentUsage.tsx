import React, { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { api, type AgentUsageRes, type QuotaWindow, type RateWindow } from '../api';

const POLL_MS = 30_000;
const FIVE_HOURS = 5 * 3600;
const SEVEN_DAYS = 7 * 86400;
/** The weekly meter joins the inline chip once it is this full (it always shows in the card). */
const WEEKLY_INLINE_PCT = 50;
/** Context fill is shown inline only while the latest conversation is this recent. */
const CTX_RECENT_SECS = 2 * 3600;

type Claude = NonNullable<AgentUsageRes['claude']>;
type Codex = NonNullable<AgentUsageRes['codex']>;

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
  if (n >= 10_000) return `${Math.round(n / 1_000)}k`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

function fmtDuration(secs: number): string {
  const s = Math.max(0, secs);
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${Math.max(1, m)}m`;
}

const nowSecs = (now: number) => Math.floor(now / 1000);

/** "in 1h 24m" for short windows, a weekday + time for long ones. */
function fmtResetLong(resetsAt: number, now: number, windowSecs: number): string {
  if (!resetsAt) return '';
  if (windowSecs <= FIVE_HOURS || resetsAt - nowSecs(now) < 86400) {
    return `Resets in ${fmtDuration(resetsAt - nowSecs(now))}`;
  }
  const d = new Date(resetsAt * 1000);
  return `Resets ${d.toLocaleString(undefined, { weekday: 'short', hour: 'numeric', minute: '2-digit' })}`;
}

function windowLabel(w: RateWindow): string {
  const mins = w.window_minutes;
  if (mins >= 1440 * 6) return '7d';
  if (mins >= 60) return `${Math.round(mins / 60)}h`;
  return mins ? `${mins}m` : 'limit';
}

function windowName(w: RateWindow): string {
  const mins = w.window_minutes;
  if (mins >= 1440 * 6) return 'Weekly';
  if (mins >= 60) return `${Math.round(mins / 60)}-hour window`;
  return 'Limit';
}

/** `claude-opus-5-5` -> `Opus 5.5`, `claude-3-5-sonnet-20241022` -> `Sonnet 3.5`. */
export function modelLabel(model: string): string {
  const m = model.toLowerCase();
  const fam = (s: string) => s[0].toUpperCase() + s.slice(1);
  let r = /claude-(opus|sonnet|haiku|fable)-(\d+)(?:-(\d{1,2}))?(?!\d)/.exec(m);
  if (r) return `${fam(r[1])} ${r[2]}${r[3] ? `.${r[3]}` : ''}`;
  r = /claude-(\d+)(?:-(\d))?-(opus|sonnet|haiku)/.exec(m);
  if (r) return `${fam(r[3])} ${r[1]}${r[2] ? `.${r[2]}` : ''}`;
  return model.replace(/\[1m\]$/i, '');
}

/** Green -> amber -> red as a budget fills up. */
function level(pct: number): string {
  if (pct >= 90) return 'var(--danger)';
  if (pct >= 70) return 'var(--modified-fg)';
  return 'var(--accent)';
}

/**
 * Projects the current burn rate to the end of the window. Returns a warning when the limit
 * would be reached before the window resets, otherwise a calm "on pace".
 */
function pace(w: QuotaWindow, windowSecs: number, now: number): { text: string; warn: boolean } | null {
  if (!w.resets_at || w.used_percent < 5) return null;
  const left = w.resets_at - nowSecs(now);
  const elapsed = windowSecs - left;
  if (left <= 0 || elapsed < windowSecs * 0.05) return null;
  if (w.used_percent >= 100) return { text: 'Limit reached', warn: true };
  const secsToFull = (elapsed * (100 - w.used_percent)) / w.used_percent;
  if (secsToFull < left) return { text: `At this rate, limit in ~${fmtDuration(secsToFull)}`, warn: true };
  return { text: 'On pace', warn: false };
}

const Meter: React.FC<{ pct: number; className?: string }> = ({ pct, className = 'w-9 h-[3px]' }) => (
  <span className={`inline-block rounded-full bg-[var(--border)] overflow-hidden shrink-0 ${className}`}>
    <span
      className="block h-full rounded-full transition-[width] duration-500"
      style={{ width: `${Math.min(100, Math.max(2, pct))}%`, background: level(pct) }}
    />
  </span>
);

const Pct: React.FC<{ pct: number }> = ({ pct }) => (
  <span style={pct >= 70 ? { color: level(pct) } : undefined}>{Math.round(pct)}%</span>
);

// ---------------------------------------------------------------------------------------------
// Card (popover)

const Popover: React.FC<{
  anchor: HTMLElement;
  onClose: () => void;
  children: React.ReactNode;
}> = ({ anchor, onClose, children }) => {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ right: number; bottom: number } | null>(null);

  useLayoutEffect(() => {
    const place = () => {
      const r = anchor.getBoundingClientRect();
      const w = ref.current?.offsetWidth ?? 320;
      const margin = 8;
      // Right-align with the chip, but never past either viewport edge.
      const right = Math.min(Math.max(margin, window.innerWidth - r.right), window.innerWidth - w - margin);
      setPos({ right: Math.max(margin, right), bottom: window.innerHeight - r.top + 6 });
    };
    place();
    window.addEventListener('resize', place);
    return () => window.removeEventListener('resize', place);
  }, [anchor]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (ref.current?.contains(t) || anchor.contains(t)) return;
      onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [anchor, onClose]);

  return createPortal(
    <div
      ref={ref}
      role="dialog"
      style={{ position: 'fixed', right: pos?.right ?? 8, bottom: pos?.bottom ?? 32, visibility: pos ? 'visible' : 'hidden' }}
      className="usage-card-in z-[70] w-[320px] max-w-[calc(100vw-16px)] rounded-xl border border-[var(--border)] bg-[var(--bg-elevated)] shadow-[0_16px_40px_-12px_rgba(0,0,0,0.55),0_2px_8px_rgba(0,0,0,0.2)] text-[12px] text-[var(--text-muted)] select-text"
    >
      {children}
    </div>,
    document.body
  );
};

const CardHeader: React.FC<{ dot: string; title: string; right?: React.ReactNode }> = ({ dot, title, right }) => (
  <div className="flex items-center justify-between gap-3 px-3.5 h-10 border-b border-[var(--border)]">
    <span className="flex items-center gap-2 font-medium text-[var(--text-primary)]">
      <span className="w-2 h-2 rounded-full" style={{ background: dot }} />
      {title}
    </span>
    {right && <span className="flex items-center gap-1.5 text-[11px] text-[var(--text-dim)] truncate">{right}</span>}
  </div>
);

const Section: React.FC<{ children: React.ReactNode; last?: boolean }> = ({ children, last }) => (
  <div className={`px-3.5 py-3 space-y-3 ${last ? '' : 'border-b border-[var(--border)]'}`}>{children}</div>
);

const MeterRow: React.FC<{
  label: string;
  pct: number;
  value: React.ReactNode;
  note?: React.ReactNode;
  noteWarn?: boolean;
}> = ({ label, pct, value, note, noteWarn }) => (
  <div>
    <div className="flex items-baseline justify-between gap-3 mb-1.5">
      <span className="text-[var(--text-primary)]">{label}</span>
      <span className="tabular-nums text-[11.5px]">{value}</span>
    </div>
    <Meter pct={pct} className="w-full h-1.5" />
    {note && (
      <div
        className="mt-1.5 text-[11px]"
        style={{ color: noteWarn ? 'var(--modified-fg)' : 'var(--text-dim)' }}
      >
        {note}
      </div>
    )}
  </div>
);

const Row: React.FC<{ label: string; children: React.ReactNode }> = ({ label, children }) => (
  <div className="flex items-baseline justify-between gap-3">
    <span>{label}</span>
    <span className="tabular-nums text-[var(--text-primary)]">{children}</span>
  </div>
);

const QUOTA_NOTE: Record<Exclude<Claude['quota_status'], 'ok'>, string> = {
  no_credentials:
    'Plan limits appear when Claude Code is signed in with a Claude subscription. Showing totals from local session logs.',
  expired:
    "Claude Code's sign-in has expired; limits return the next time Claude Code runs. Showing totals from local session logs.",
  error: "Couldn't reach Claude's usage service. Showing totals from local session logs.",
};

function ClaudeCard({ c, now }: { c: Claude; now: number }) {
  const q = c.quota;
  const ctxPct = c.ctx_window ? (c.ctx_tokens / c.ctx_window) * 100 : 0;
  const model = c.model ? modelLabel(c.model) : '';
  const plan = c.plan ? c.plan[0].toUpperCase() + c.plan.slice(1) : '';
  const five = q?.five_hour;
  const week = q?.seven_day;
  const fivePace = five ? pace(five, FIVE_HOURS, now) : null;
  const weekPace = week ? pace(week, SEVEN_DAYS, now) : null;
  const split = [
    q?.seven_day_sonnet && `Sonnet ${Math.round(q.seven_day_sonnet.used_percent)}%`,
    q?.seven_day_opus && `Opus ${Math.round(q.seven_day_opus.used_percent)}%`,
  ].filter(Boolean);

  return (
    <>
      <CardHeader
        dot="#d97757"
        title={plan ? `Claude Code · ${plan}` : 'Claude Code'}
        right={
          model && (
            <>
              {model}
              {c.ctx_window >= 1_000_000 && (
                <span className="px-1 rounded border border-[var(--border)] text-[10px]">1M</span>
              )}
            </>
          )
        }
      />
      {q ? (
        <Section>
          {five && (
            <MeterRow
              label="5-hour session"
              pct={five.used_percent}
              value={
                <>
                  <Pct pct={five.used_percent} />
                  {five.resets_at > 0 && <span className="text-[var(--text-dim)]"> · {fmtResetLong(five.resets_at, now, FIVE_HOURS)}</span>}
                </>
              }
              note={[fivePace?.text, c.block_tokens > 0 && `${fmtTokens(c.block_tokens)} tokens`].filter(Boolean).join(' · ')}
              noteWarn={fivePace?.warn}
            />
          )}
          {week && (
            <MeterRow
              label="Weekly"
              pct={week.used_percent}
              value={
                <>
                  <Pct pct={week.used_percent} />
                  {week.resets_at > 0 && <span className="text-[var(--text-dim)]"> · {fmtResetLong(week.resets_at, now, SEVEN_DAYS)}</span>}
                </>
              }
              note={[weekPace?.warn ? weekPace.text : '', ...split].filter(Boolean).join(' · ')}
              noteWarn={weekPace?.warn}
            />
          )}
        </Section>
      ) : (
        <Section>
          <Row label="5-hour block (local logs)">
            {c.block_resets_at > 0 ? (
              <>
                {fmtTokens(c.block_tokens)} tokens
                <span className="text-[var(--text-dim)]"> · resets in {fmtDuration(c.block_resets_at - nowSecs(now))}</span>
              </>
            ) : (
              <span className="text-[var(--text-dim)]">No active block</span>
            )}
          </Row>
          <p className="text-[11px] leading-relaxed text-[var(--text-dim)]">
            {QUOTA_NOTE[c.quota_status === 'ok' ? 'error' : c.quota_status] ?? QUOTA_NOTE.error}
          </p>
        </Section>
      )}
      {c.ctx_tokens > 0 && (
        <Section>
          <MeterRow
            label="Context window"
            pct={ctxPct}
            value={
              <>
                {fmtTokens(c.ctx_tokens)} / {fmtTokens(c.ctx_window)}{' '}
                <span className="text-[var(--text-dim)]">({Math.round(ctxPct)}%)</span>
              </>
            }
            note={
              c.ctx_updated_at
                ? `Latest conversation in this workspace · ${fmtDuration(nowSecs(now) - c.ctx_updated_at)} ago`
                : 'Latest conversation in this workspace'
            }
          />
        </Section>
      )}
      <Section last>
        <Row label="Last 24h in this workspace">{fmtTokens(c.day_tokens)} tokens</Row>
        {c.day_tokens > 0 && <Row label="Prompt cache hits">{Math.round(c.cache_hit * 100)}%</Row>}
      </Section>
    </>
  );
}

function CodexCard({ c, now }: { c: Codex; now: number }) {
  const windows = [c.primary, c.secondary].filter((w): w is RateWindow => !!w);
  const ctxPct = c.ctx_window ? (c.ctx_tokens / c.ctx_window) * 100 : 0;
  return (
    <>
      <CardHeader dot="#10a37f" title="Codex" />
      {windows.length > 0 && (
        <Section>
          {windows.map((w) => {
            const secs = w.window_minutes * 60;
            const p = secs ? pace(w, secs, now) : null;
            return (
              <MeterRow
                key={w.window_minutes}
                label={windowName(w)}
                pct={w.used_percent}
                value={
                  <>
                    <Pct pct={w.used_percent} />
                    {w.resets_at > 0 && <span className="text-[var(--text-dim)]"> · {fmtResetLong(w.resets_at, now, secs)}</span>}
                  </>
                }
                note={p?.text}
                noteWarn={p?.warn}
              />
            );
          })}
        </Section>
      )}
      {c.ctx_window > 0 && c.ctx_tokens > 0 && (
        <Section>
          <MeterRow
            label="Context window"
            pct={ctxPct}
            value={
              <>
                {fmtTokens(c.ctx_tokens)} / {fmtTokens(c.ctx_window)}{' '}
                <span className="text-[var(--text-dim)]">({Math.round(ctxPct)}%)</span>
              </>
            }
          />
        </Section>
      )}
      <Section last>
        <Row label="Last 24h in this workspace">{fmtTokens(c.day_tokens)} tokens</Row>
      </Section>
    </>
  );
}

// ---------------------------------------------------------------------------------------------
// Inline chips

interface ChipProps {
  name: string;
  dot: string;
  label: string;
  open: boolean;
  onToggle: (el: HTMLElement) => void;
  children: React.ReactNode;
}

const Chip: React.FC<ChipProps> = ({ name, dot, label, open, onToggle, children }) => (
  <button
    type="button"
    onClick={(e) => onToggle(e.currentTarget)}
    aria-haspopup="dialog"
    aria-expanded={open}
    aria-label={label}
    className={`flex items-center gap-2 px-2 h-[20px] my-auto rounded-md tabular-nums transition-colors cursor-pointer ${
      open
        ? 'bg-[var(--bg-hover)] text-[var(--text-primary)]'
        : 'text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
    }`}
  >
    <span className="flex items-center gap-1.5 font-medium text-[var(--text-primary)]">
      <span className="w-1.5 h-1.5 rounded-full" style={{ background: dot }} />
      {name}
    </span>
    {children}
  </button>
);

const Sep = () => <span className="opacity-40">·</span>;
const Tag: React.FC<{ children: React.ReactNode }> = ({ children }) => <span className="opacity-60">{children}</span>;

function ClaudeInline({ c, now }: { c: Claude; now: number }) {
  const q = c.quota;
  const ctxPct = c.ctx_window ? (c.ctx_tokens / c.ctx_window) * 100 : 0;
  const ctxRecent = c.ctx_tokens > 0 && c.ctx_updated_at > 0 && nowSecs(now) - c.ctx_updated_at <= CTX_RECENT_SECS;

  if (q && (q.five_hour || q.seven_day)) {
    const five = q.five_hour;
    const week = q.seven_day;
    const showWeek = week && (!five || week.used_percent >= WEEKLY_INLINE_PCT);
    return (
      <>
        {five && (
          <span className="flex items-center gap-1.5">
            <Meter pct={five.used_percent} />
            <Pct pct={five.used_percent} />
            <Tag>5h</Tag>
            {five.resets_at > nowSecs(now) && (
              <>
                <Sep />
                <span>↻ {fmtDuration(five.resets_at - nowSecs(now))}</span>
              </>
            )}
          </span>
        )}
        {showWeek && (
          <span className="flex items-center gap-1.5">
            <Meter pct={week.used_percent} />
            <Pct pct={week.used_percent} />
            <Tag>7d</Tag>
          </span>
        )}
        {ctxRecent && (
          <span className="flex items-center gap-1">
            <Tag>ctx</Tag>
            <Pct pct={ctxPct} />
          </span>
        )}
      </>
    );
  }

  // No plan limits (API key, signed out, offline): label everything so the context meter can't be
  // mistaken for quota.
  return (
    <>
      {ctxRecent && (
        <span className="flex items-center gap-1.5">
          <Tag>ctx</Tag>
          <Meter pct={ctxPct} />
          <Pct pct={ctxPct} />
        </span>
      )}
      {c.block_resets_at > nowSecs(now) ? (
        <span className="flex items-center gap-1.5">
          <Tag>5h</Tag>
          {fmtTokens(c.block_tokens)}
          <Sep />
          <span>↻ {fmtDuration(c.block_resets_at - nowSecs(now))}</span>
        </span>
      ) : (
        <span>
          {fmtTokens(c.day_tokens)} <Tag>today</Tag>
        </span>
      )}
    </>
  );
}

function CodexInline({ c }: { c: Codex }) {
  const windows = [c.primary, c.secondary].filter((w): w is RateWindow => !!w);
  if (windows.length === 0) {
    return (
      <span>
        {fmtTokens(c.day_tokens)} <Tag>today</Tag>
      </span>
    );
  }
  return (
    <>
      {windows.map((w) => (
        <span key={w.window_minutes} className="flex items-center gap-1.5">
          <Meter pct={w.used_percent} />
          <Pct pct={w.used_percent} />
          <Tag>{windowLabel(w)}</Tag>
        </span>
      ))}
    </>
  );
}

/** Per-agent usage in the status bar; click a chip for the detail card. */
export const AgentUsage: React.FC = () => {
  const [usage, setUsage] = useState<AgentUsageRes | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [open, setOpen] = useState<{ which: 'claude' | 'codex'; el: HTMLElement } | null>(null);

  useEffect(() => {
    let alive = true;
    const load = () => {
      if (document.hidden) return;
      api
        .usage()
        .then((u) => {
          if (!alive) return;
          setUsage(u);
          setNow(Date.now());
        })
        .catch(() => {});
    };
    load();
    const t = window.setInterval(load, POLL_MS);
    document.addEventListener('visibilitychange', load);
    return () => {
      alive = false;
      window.clearInterval(t);
      document.removeEventListener('visibilitychange', load);
    };
  }, []);

  // Keep countdowns honest while the card is open.
  useEffect(() => {
    if (!open) return;
    setNow(Date.now());
    const t = window.setInterval(() => setNow(Date.now()), 15_000);
    return () => window.clearInterval(t);
  }, [open]);

  const close = React.useCallback(() => setOpen(null), []);

  if (!usage || (!usage.claude && !usage.codex)) return null;
  const { claude, codex } = usage;
  const toggle = (which: 'claude' | 'codex') => (el: HTMLElement) =>
    setOpen((cur) => (cur?.which === which ? null : { which, el }));

  return (
    <div className="sb-group flex items-center gap-0.5 min-w-0">
      {claude && (
        <Chip
          name="Claude"
          dot="#d97757"
          label="Claude Code usage"
          open={open?.which === 'claude'}
          onToggle={toggle('claude')}
        >
          <ClaudeInline c={claude} now={now} />
        </Chip>
      )}
      {codex && (
        <Chip name="Codex" dot="#10a37f" label="Codex usage" open={open?.which === 'codex'} onToggle={toggle('codex')}>
          <CodexInline c={codex} />
        </Chip>
      )}
      {open?.which === 'claude' && claude && (
        <Popover anchor={open.el} onClose={close}>
          <ClaudeCard c={claude} now={now} />
        </Popover>
      )}
      {open?.which === 'codex' && codex && (
        <Popover anchor={open.el} onClose={close}>
          <CodexCard c={codex} now={now} />
        </Popover>
      )}
    </div>
  );
};
