import React, { useEffect, useState } from 'react';
import { api, type AgentUsageRes, type RateWindow } from '../api';

const POLL_MS = 30_000;

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
  return String(n);
}

function fmtReset(epochSecs: number, now: number): string {
  const s = Math.max(0, epochSecs - Math.floor(now / 1000));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${Math.max(1, m)}m`;
}

function windowLabel(w: RateWindow): string {
  const mins = w.window_minutes;
  if (mins >= 1440 * 6) return 'weekly';
  if (mins >= 60) return `${Math.round(mins / 60)}h`;
  return mins ? `${mins}m` : 'limit';
}

/** Green -> amber -> red as a budget fills up. */
function level(pct: number): string {
  if (pct >= 90) return 'var(--danger)';
  if (pct >= 70) return 'var(--modified-fg)';
  return 'var(--accent)';
}

const Meter: React.FC<{ pct: number }> = ({ pct }) => (
  <span className="inline-block w-9 h-[3px] rounded-full bg-[var(--border)] overflow-hidden shrink-0">
    <span
      className="block h-full rounded-full"
      style={{ width: `${Math.min(100, Math.max(2, pct))}%`, background: level(pct) }}
    />
  </span>
);

interface ChipProps {
  name: string;
  dot: string;
  title: string;
  children: React.ReactNode;
}

const Chip: React.FC<ChipProps> = ({ name, dot, title, children }) => (
  <span
    className="flex items-center gap-2 px-2.5 h-[20px] my-auto rounded-md tabular-nums text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
    title={title}
  >
    <span className="flex items-center gap-1.5 font-medium text-[var(--text-primary)]">
      <span className="w-1.5 h-1.5 rounded-full" style={{ background: dot }} />
      {name}
    </span>
    {children}
  </span>
);

/** Per-agent usage in the status bar, read from the agents' own session logs. */
export const AgentUsage: React.FC = () => {
  const [usage, setUsage] = useState<AgentUsageRes | null>(null);
  const [now, setNow] = useState(() => Date.now());

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

  if (!usage || (!usage.claude && !usage.codex)) return null;
  const { claude, codex } = usage;

  return (
    <div className="flex items-center gap-0.5 min-w-0">
      {claude && (
        <Chip
          name="Claude"
          dot="#d97757"
          title={[
            `Claude Code${claude.model ? ` (${claude.model})` : ''}`,
            claude.block_resets_at
              ? `5-hour block: ${fmtTokens(claude.block_tokens)} tokens, resets in ${fmtReset(claude.block_resets_at, now)}`
              : 'No active 5-hour block',
            `Last 24h in this workspace: ${fmtTokens(claude.day_tokens)} tokens`,
            claude.ctx_tokens
              ? `Context: ${claude.ctx_tokens.toLocaleString()} / ${claude.ctx_window.toLocaleString()} tokens`
              : '',
          ]
            .filter(Boolean)
            .join('\n')}
        >
          {claude.ctx_tokens > 0 && (
            <span className="flex items-center gap-1.5">
              <Meter pct={(claude.ctx_tokens / claude.ctx_window) * 100} />
              {Math.round((claude.ctx_tokens / claude.ctx_window) * 100)}%
            </span>
          )}
          {claude.block_resets_at > 0 ? (
            <span>
              {fmtTokens(claude.block_tokens)} · resets {fmtReset(claude.block_resets_at, now)}
            </span>
          ) : (
            <span>{fmtTokens(claude.day_tokens)} today</span>
          )}
        </Chip>
      )}
      {codex && (
        <Chip
          name="Codex"
          dot="#10a37f"
          title={[
            'Codex',
            ...[codex.primary, codex.secondary]
              .filter((w): w is RateWindow => !!w)
              .map(
                (w) =>
                  `${windowLabel(w)} limit: ${Math.round(w.used_percent)}% used${
                    w.resets_at ? `, resets in ${fmtReset(w.resets_at, now)}` : ''
                  }`
              ),
            `Last 24h in this workspace: ${fmtTokens(codex.day_tokens)} tokens`,
            codex.ctx_window ? `Context: ${codex.ctx_tokens.toLocaleString()} / ${codex.ctx_window.toLocaleString()} tokens` : '',
          ]
            .filter(Boolean)
            .join('\n')}
        >
          {[codex.primary, codex.secondary]
            .filter((w): w is RateWindow => !!w)
            .map((w) => (
              <span key={w.window_minutes} className="flex items-center gap-1.5">
                <Meter pct={w.used_percent} />
                {Math.round(w.used_percent)}%
                <span className="opacity-70">{windowLabel(w)}</span>
              </span>
            ))}
          {!codex.primary && !codex.secondary && <span>{fmtTokens(codex.day_tokens)} today</span>}
        </Chip>
      )}
    </div>
  );
};
