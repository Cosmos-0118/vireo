import React, { useEffect, useState } from 'react';
import { Check, ChevronLeft, ChevronRight, Copy, X } from 'lucide-react';
import type { PermissionRequest } from '../../types';

/** Human-readable scope of a saved "always allow" rule (mirrors `permission_rule` in the backend). */
export function describeRule(rule: string): string {
  const [tool, kind, ...rest] = rule.split(':');
  const value = rest.join(':');
  if (kind === 'cmd') return `${tool} commands starting with "${value}" (no pipes or chaining)`;
  if (kind === 'dir') return `${tool} on any file under ${value}/`;
  return `Every ${rule} request`;
}

const SHELL_TOOLS = new Set(['Bash', 'PowerShell']);

const str = (v: unknown) => (typeof v === 'string' ? v.trim() : '');

/** What the request is, in plain words: category prefix, the agent's stated intent, and the exact target. */
export function describeRequest(p: PermissionRequest) {
  const input = p.input ?? {};
  const intent = str(input.description) || str(input.reason) || str(input.title);
  const command = str(input.command);
  const target = str(input.file_path) || str(input.path);
  const shell = SHELL_TOOLS.has(p.tool);
  const prefix = shell ? 'Run in terminal' : target ? `${p.tool === 'Write' ? 'Write' : 'Edit'} file` : `Run ${p.tool}`;
  // `summary` is "<Tool> <detail>"; drop the tool name so the code well shows only the detail.
  const fallback = p.summary.startsWith(`${p.tool} `) ? p.summary.slice(p.tool.length + 1) : p.summary;
  const code = shell ? command || fallback : target || fallback;
  return { prefix, intent, code };
}

const iconBtn =
  'flex items-center justify-center w-5 h-5 rounded text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] disabled:opacity-30 disabled:pointer-events-none cursor-pointer';
const secondaryBtn =
  'h-6 px-2.5 rounded text-[11.5px] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer';

interface PermissionCardProps {
  /** Every pending request, oldest first; the card pages through them. */
  requests: PermissionRequest[];
  onAnswer: (request: PermissionRequest, allow: boolean, remember?: boolean) => void;
  onAllowAll: () => void;
}

/** Docked approval tray above the composer: one request at a time with `1/N` paging and batch allow. */
export const PermissionCard: React.FC<PermissionCardProps> = ({ requests, onAnswer, onAllowAll }) => {
  const [index, setIndex] = useState(0);
  const [details, setDetails] = useState(false);
  const [copied, setCopied] = useState(false);
  const total = requests.length;
  const at = Math.min(index, Math.max(0, total - 1));
  const p = requests[at];
  const id = p?.request_id;

  useEffect(() => {
    setDetails(false);
    setCopied(false);
  }, [id]);

  if (!p) return null;
  const { prefix, intent, code } = describeRequest(p);

  return (
    <div className="mx-3 mb-2 rounded-lg border border-[var(--border)] bg-[var(--bg-panel)] shadow-lg overflow-hidden">
      <div className="flex items-start justify-between gap-2 px-3 py-2 border-b border-[var(--border)]">
        <div className="min-w-0 text-[12px] leading-[1.35]">
          <span className="font-medium text-[var(--text-primary)]">{prefix}</span>
          {intent && (
            <>
              <span className="text-[var(--text-primary)]">: </span>
              <span className="text-[var(--text-muted)]">{intent}</span>
            </>
          )}
        </div>
        <div className="flex items-center gap-1 shrink-0">
          {total > 1 && (
            <>
              <button className={iconBtn} disabled={at === 0} onClick={() => setIndex(at - 1)} title="Previous">
                <ChevronLeft size={12} />
              </button>
              <span className="text-[11px] tabular-nums text-[var(--text-muted)]">
                {at + 1}/{total}
              </span>
              <button className={iconBtn} disabled={at >= total - 1} onClick={() => setIndex(at + 1)} title="Next">
                <ChevronRight size={12} />
              </button>
              <button
                onClick={onAllowAll}
                className="h-5 px-2 ml-0.5 rounded bg-[var(--accent)] text-white text-[11px] font-medium cursor-pointer hover:opacity-95"
                title={`Allow all ${total} pending requests`}
              >
                Allow All
              </button>
            </>
          )}
          <button className={iconBtn} onClick={() => onAnswer(p, false)} title="Skip">
            <X size={13} />
          </button>
        </div>
      </div>

      <div className="group/code relative px-3 py-2.5 bg-[var(--bg-input)]/60 font-mono text-[12px] leading-[1.5] max-h-36 overflow-y-auto whitespace-pre-wrap break-words text-[var(--text-primary)]">
        {code}
        <button
          onClick={() => {
            void navigator.clipboard?.writeText(code).then(() => {
              setCopied(true);
              setTimeout(() => setCopied(false), 1200);
            });
          }}
          className="absolute top-1 right-1 flex items-center justify-center w-5 h-5 rounded bg-[var(--bg-elevated)] text-[var(--text-dim)] hover:text-[var(--text-primary)] opacity-0 group-hover/code:opacity-100 transition-opacity cursor-pointer"
          title={copied ? 'Copied' : 'Copy'}
        >
          {copied ? <Check size={11} /> : <Copy size={11} />}
        </button>
      </div>

      {details && (
        <div className="px-3 py-2 border-t border-[var(--border)]/60 text-[11px] leading-[1.5] text-[var(--text-muted)] space-y-0.5">
          <div>
            Tool: <span className="font-mono text-[var(--text-primary)]">{p.tool}</span>
          </div>
          <div>“Always allow” covers: {describeRule(p.rule)}</div>
          {!intent && <div>The agent gave no reason for this request.</div>}
        </div>
      )}

      <div className="flex items-center gap-1.5 px-3 py-2 border-t border-[var(--border)]/60">
        <button
          onClick={() => onAnswer(p, true)}
          className="h-6 px-2.5 rounded text-[11.5px] font-medium bg-[var(--accent)] text-white cursor-pointer hover:opacity-95"
        >
          Allow Once
        </button>
        <button onClick={() => onAnswer(p, false)} className={secondaryBtn}>
          Skip
        </button>
        <button onClick={() => setDetails((v) => !v)} className={secondaryBtn} aria-expanded={details}>
          Details
        </button>
        <span className="flex-1" />
        <button
          onClick={() => onAnswer(p, true, true)}
          className="h-6 px-1.5 rounded text-[11px] text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
          title={`For the rest of this thread, allow: ${describeRule(p.rule)}`}
        >
          Always allow
        </button>
      </div>
    </div>
  );
};
