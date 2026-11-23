import React from 'react';
import { Lock } from 'lucide-react';
import type { PermissionRequest } from '../../types';

/** Human-readable scope of a saved "always allow" rule (mirrors `permission_rule` in the backend). */
export function describeRule(rule: string): string {
  const [tool, kind, ...rest] = rule.split(':');
  const value = rest.join(':');
  if (kind === 'cmd') return `${tool} commands starting with "${value}" (no pipes or chaining)`;
  if (kind === 'dir') return `${tool} on any file under ${value}/`;
  return `Every ${rule} request`;
}

interface PermissionCardProps {
  request: PermissionRequest;
  onAnswer: (request: PermissionRequest, allow: boolean, remember?: boolean) => void;
}

/** In-chat approval prompt for a tool call the agent wants to make. */
export const PermissionCard: React.FC<PermissionCardProps> = ({ request: p, onAnswer }) => (
  <div className="rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] px-3 py-2 space-y-1.5">
    <div className="flex items-center gap-1.5 text-[11px] text-[var(--text-muted)]">
      <Lock size={11} className="shrink-0" />
      Permission requested · {p.tool}
    </div>
    <div className="text-[12px] font-mono text-[var(--text-primary)] break-all max-h-24 overflow-y-auto">
      {p.summary}
    </div>
    <div className="flex flex-wrap gap-1.5 pt-0.5">
      <button
        onClick={() => onAnswer(p, true)}
        className="h-6 px-2 rounded-md text-[11px] bg-[var(--accent)] text-white cursor-pointer hover:opacity-95"
      >
        Allow
      </button>
      <button
        onClick={() => onAnswer(p, true, true)}
        className="h-6 px-2 rounded-md text-[11px] border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
        title={`For the rest of this thread, allow: ${describeRule(p.rule)}`}
      >
        Always allow in thread
      </button>
      <button
        onClick={() => onAnswer(p, false)}
        className="h-6 px-2 rounded-md text-[11px] text-[var(--text-muted)] hover:bg-[var(--bg-hover)] cursor-pointer"
      >
        Deny
      </button>
    </div>
  </div>
);
