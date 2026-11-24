import React from 'react';
import { Bot, ChevronRight, ExternalLink, FileText, Package, Rocket, ShieldCheck, SquareTerminal, User } from 'lucide-react';
import type { ReleaseActivity } from '../../types';
import { StatusIcon, TONE, relTime, tint } from './ui';

const SOURCE: Record<ReleaseActivity['source'], { label: string; icon: React.ReactNode }> = {
  ui: { label: 'You', icon: <User size={11} /> },
  agent: { label: 'Agent', icon: <Bot size={11} /> },
  cli: { label: 'Terminal', icon: <SquareTerminal size={11} /> },
};

export function activityTitle(e: ReleaseActivity): string {
  const tag = e.tag ?? e.version ?? '';
  switch (e.action) {
    case 'preflight':
      return `Ran readiness checks${tag ? ` for ${tag}` : ''}`;
    case 'notes':
      return `Wrote release notes${tag ? ` for ${tag}` : ''}`;
    case 'bump':
      return `Bumped versions${e.version ? ` to ${e.version}` : ''}`;
    case 'verify':
      return `Verified ${tag}`;
    case 'publish':
      if (e.phase === 'running') return `Releasing ${tag}…`;
      if (e.dry_run) return `Planned release ${tag}`;
      return e.ok ? `Released ${tag}` : `Release ${tag} stopped`;
  }
}

function ActionIcon({ e }: { e: ReleaseActivity }) {
  const size = 13;
  switch (e.action) {
    case 'preflight':
    case 'verify':
      return <ShieldCheck size={size} />;
    case 'notes':
      return <FileText size={size} />;
    case 'bump':
      return <Package size={size} />;
    case 'publish':
      return <Rocket size={size} />;
  }
}

/** Timeline of release actions by you, agents and the terminal; newest first. */
export function ActivityFeed({
  items,
  onOpen,
  onOpenFile,
  onOpenUrl,
}: {
  items: ReleaseActivity[];
  onOpen: (e: ReleaseActivity) => void;
  onOpenFile: (path: string) => void;
  onOpenUrl: (url: string) => void;
}) {
  if (items.length === 0) {
    return (
      <p className="px-4 py-5 text-[12px] text-[var(--text-muted)] leading-relaxed">
        Nothing yet. Checks, notes and releases show up here as they happen, including ones an agent runs.
      </p>
    );
  }
  return (
    <ul className="flex flex-col">
      {items.map((e) => {
        const src = SOURCE[e.source] ?? SOURCE.cli;
        const status = e.phase === 'running' ? 'running' : e.ok === false ? 'fail' : e.ok ? 'ok' : 'skip';
        const clickable = !!e.detail;
        return (
          <li key={e.id} className="border-t border-[var(--border)] first:border-t-0">
            <div
              role={clickable ? 'button' : undefined}
              tabIndex={clickable ? 0 : undefined}
              onClick={clickable ? () => onOpen(e) : undefined}
              onKeyDown={clickable ? (k) => k.key === 'Enter' && onOpen(e) : undefined}
              className={`group flex gap-3 px-4 py-2.5 ${clickable ? 'cursor-pointer hover:bg-[var(--bg-hover)]' : ''}`}
            >
              <span
                className="mt-0.5 w-6 h-6 rounded-md flex items-center justify-center shrink-0"
                style={{
                  color: e.phase === 'running' ? TONE.accent : 'var(--text-muted)',
                  background: e.phase === 'running' ? tint('accent', 16) : 'var(--bg-active)',
                }}
              >
                <ActionIcon e={e} />
              </span>
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-1.5">
                  <span className="text-[12.5px] text-[var(--text-primary)] truncate">{activityTitle(e)}</span>
                  <StatusIcon status={status} size={13} />
                </div>
                {e.summary && <div className="text-[11.5px] text-[var(--text-muted)] leading-snug break-words">{e.summary}</div>}
                <div className="mt-1 flex items-center gap-2 text-[11px] text-[var(--text-dim)]">
                  <span
                    className="inline-flex items-center gap-1"
                    style={e.source === 'agent' ? { color: TONE.accent } : undefined}
                  >
                    {src.icon}
                    {src.label}
                  </span>
                  <span>·</span>
                  <span title={new Date(e.ts).toLocaleString()}>{relTime(e.ts)}</span>
                  {e.path && (
                    <button
                      className="ml-auto hover:text-[var(--text-primary)] hover:underline cursor-pointer"
                      onClick={(ev) => {
                        ev.stopPropagation();
                        onOpenFile(e.path!);
                      }}
                    >
                      Open file
                    </button>
                  )}
                  {e.url && (
                    <button
                      className="ml-auto inline-flex items-center gap-1 hover:text-[var(--text-primary)] cursor-pointer"
                      onClick={(ev) => {
                        ev.stopPropagation();
                        onOpenUrl(e.url!);
                      }}
                    >
                      GitHub <ExternalLink size={10} />
                    </button>
                  )}
                </div>
              </div>
              {clickable && (
                <ChevronRight size={14} className="mt-1 shrink-0 text-[var(--text-dim)] opacity-0 group-hover:opacity-100" />
              )}
            </div>
          </li>
        );
      })}
    </ul>
  );
}
