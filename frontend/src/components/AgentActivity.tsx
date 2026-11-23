import React, { useEffect, useState } from 'react';
import { visibleInterval } from '../utils/visibleInterval';
import {
  AlertCircle,
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  FilePlus2,
  FileText,
  FolderOpen,
  FolderSearch,
  Globe,
  ListChecks,
  ListTree,
  Loader2,
  Plug,
  Search,
  ShieldAlert,
  Sparkles,
  SquarePen,
  Terminal as TerminalIcon,
  Wrench,
} from 'lucide-react';
import type { GitFileStatus, ToolCallTrace } from '../types';

// ---------------------------------------------------------------------------------------------
// Helpers shared with AgentPanel
// ---------------------------------------------------------------------------------------------

export const baseName = (p: string) => p.split(/[\\/]/).pop() || p;
export const normPath = (p: string) => p.replace(/\\/g, '/').replace(/^\.\//, '');
export const formatDur = (ms: number) =>
  ms < 60_000 ? `${Math.round(ms / 100) / 10}s` : `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`;

export type ChangeLookup = (path: string) => GitFileStatus | undefined;

type IconT = React.ComponentType<{ size?: number; className?: string; strokeWidth?: number }>;

type ToolKind = 'read' | 'search' | 'edit' | 'command' | 'web' | 'plan' | 'task' | 'other';

export interface ToolView {
  Icon: IconT;
  kind: ToolKind;
  /** Past tense, shown once the step finished ("Read"). */
  verb: string;
  /** Present progressive, shown while the step runs ("Reading"). */
  active: string;
  /** Human-readable purpose the agent gave the step ("Show working tree status"). Leads the row when present. */
  intent?: string;
  /** Main object of the step: file name, command, query. Rendered in a mono pill. */
  subject: string;
  /** Full form of the subject for tooltips. */
  title?: string;
  /** Secondary detail: line range, symbol, server name. */
  meta?: string;
  /** Workspace file this call touched (clicking an edit opens it in the diff viewer). */
  path?: string;
  edits: boolean;
  /** Custom expanded body (otherwise the raw result is shown). */
  detail?: string;
}

const humanize = (s: string) => {
  const w = s
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .replace(/[_-]+/g, ' ')
    .trim()
    .toLowerCase();
  return w ? w[0].toUpperCase() + w.slice(1) : s;
};

const firstLine = (s: string) => s.split(/\r?\n/).find((l) => l.trim())?.trim() ?? '';

const hostOf = (url: string) => {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
};

export function describeTool(t: ToolCallTrace): ToolView {
  const a = t.args ?? {};
  const str = (k: string) => (typeof a[k] === 'string' ? (a[k] as string) : '');
  const num = (k: string) => (typeof a[k] === 'number' ? (a[k] as number) : undefined);
  const file = (path: string, extra: Partial<ToolView> & Pick<ToolView, 'Icon' | 'kind' | 'verb' | 'active'>) => ({
    subject: baseName(path) || 'file',
    title: path,
    path: path || undefined,
    edits: false,
    ...extra,
  });

  switch (t.name) {
    case 'read_file': {
      const s = num('start_line');
      const e = num('end_line');
      return file(str('path'), {
        Icon: FileText,
        kind: 'read',
        verb: 'Read',
        active: 'Reading',
        meta: str('symbol') || (s || e ? `L${s ?? 1}–${e ?? 'end'}` : undefined),
      });
    }
    case 'outline':
      return file(str('path'), { Icon: ListTree, kind: 'read', verb: 'Outlined', active: 'Outlining' });
    case 'repo_map':
      return {
        Icon: FolderSearch,
        kind: 'search',
        verb: 'Mapped',
        active: 'Mapping',
        subject: str('query') || str('dir') || 'repository',
        edits: false,
      };
    case 'symbol_edit': {
      const mode = str('mode');
      const mut = mode === 'replace' || mode === 'insert_after' || mode === 'delete';
      return file(str('path'), {
        Icon: mut ? SquarePen : FileText,
        kind: mut ? 'edit' : 'read',
        verb: mut ? 'Edited' : 'Read',
        active: mut ? 'Editing' : 'Reading',
        meta: str('symbol') || undefined,
        edits: mut,
      });
    }
    case 'ast': {
      const rw = !!str('rewrite');
      return {
        Icon: rw ? SquarePen : Search,
        kind: rw ? 'edit' : 'search',
        verb: rw ? 'Rewrote' : 'Matched',
        active: rw ? 'Rewriting' : 'Matching',
        subject: str('pattern'),
        meta: 'ast',
        edits: rw,
      };
    }
    case 'verify':
      return {
        Icon: TerminalIcon,
        kind: 'command',
        verb: 'Verified',
        active: 'Verifying',
        subject: str('command') || 'build',
        edits: false,
      };
    case 'search_code':
      return { Icon: Search, kind: 'search', verb: 'Searched', active: 'Searching', subject: str('query'), edits: false };
    case 'list_files': {
      const dir = str('dir');
      const glob = /[*?[{]/.test(dir);
      return {
        Icon: glob ? FolderSearch : FolderOpen,
        kind: 'search',
        verb: glob ? 'Found files' : 'Listed',
        active: glob ? 'Finding files' : 'Listing',
        subject: dir || 'workspace',
        edits: false,
      };
    }
    case 'edit_file':
      return file(str('path'), { Icon: SquarePen, kind: 'edit', verb: 'Edited', active: 'Editing', edits: true });
    case 'write_file':
      return file(str('path'), {
        Icon: FilePlus2,
        kind: 'edit',
        verb: str('kind') === 'add' ? 'Created' : 'Wrote',
        active: 'Writing',
        edits: true,
      });
    case 'run_command':
      return {
        Icon: TerminalIcon,
        kind: 'command',
        verb: 'Ran',
        active: 'Running',
        intent: str('description').trim() || undefined,
        subject: firstLine(str('command')),
        title: str('command'),
        edits: false,
      };
    case 'TodoWrite': {
      const todos = Array.isArray(a.todos) ? (a.todos as { content?: string; status?: string }[]) : [];
      const done = todos.filter((x) => x.status === 'completed').length;
      return {
        Icon: ListChecks,
        kind: 'plan',
        verb: 'Updated plan',
        active: 'Updating plan',
        subject: '',
        meta: todos.length ? `${done}/${todos.length} done` : undefined,
        edits: false,
        detail: todos
          .map((x) => `${x.status === 'completed' ? '✓' : x.status === 'in_progress' ? '›' : '○'} ${x.content ?? ''}`)
          .join('\n'),
      };
    }
    case 'WebFetch':
      return {
        Icon: Globe,
        kind: 'web',
        verb: 'Fetched',
        active: 'Fetching',
        subject: hostOf(str('url')),
        title: str('url'),
        edits: false,
      };
    case 'WebSearch':
      return { Icon: Globe, kind: 'web', verb: 'Searched web', active: 'Searching web', subject: str('query'), edits: false };
    case 'Task':
    case 'Agent':
      return {
        Icon: Bot,
        kind: 'task',
        verb: 'Delegated',
        active: 'Delegating',
        subject: str('description') || str('subagent_type') || 'subtask',
        edits: false,
      };
    case 'Skill':
      return { Icon: Sparkles, kind: 'other', verb: 'Used skill', active: 'Using skill', subject: str('skill') || str('name'), edits: false };
    case 'ToolSearch':
      return { Icon: Wrench, kind: 'other', verb: 'Loaded tools', active: 'Loading tools', subject: str('query'), edits: false };
    default: {
      const mcp = /^mcp__(.+?)__(.+)$/.exec(t.name);
      const raw = mcp ? mcp[2] : t.name;
      const subject =
        str('path') || str('command') || str('pattern') || str('query') || str('url') || str('description') || str('file_path');
      const verb = humanize(raw);
      return {
        Icon: mcp ? Plug : Wrench,
        kind: 'other',
        verb,
        active: verb,
        subject: str('path') || str('file_path') ? baseName(subject) : firstLine(subject),
        title: subject,
        meta: mcp ? humanize(mcp[1]) : undefined,
        edits: false,
      };
    }
  }
}

export type ToolStatus = 'running' | 'ok' | 'failed' | 'blocked' | 'stopped';

const EXIT_RE = /^exit=(-?\d+)/;
const BLOCKED_RE = /permission|denied|not allowed|requires approval|blocked|haven'?t granted|was rejected|user rejected/i;

export const isFailed = (t: ToolCallTrace) => {
  if (t.result === undefined) return false;
  const r = t.result.trim();
  if (/^error/i.test(r)) return true;
  const exit = EXIT_RE.exec(r);
  if (!exit || exit[1] === '0') return false;
  // rg / grep / findstr exit 1 with no output when nothing matched: that is an answer, not a failure.
  return !(exit[1] === '1' && !r.slice(exit[0].length).trim());
};

export function toolStatus(t: ToolCallTrace, turnActive: boolean): ToolStatus {
  if (t.result === undefined) return turnActive ? 'running' : 'stopped';
  if (!isFailed(t)) return 'ok';
  return BLOCKED_RE.test(t.result.slice(0, 400)) ? 'blocked' : 'failed';
}

/** Remove `<tag>` / `</tag>` markers, repeating until stable so nested input like `<scr<x>ipt>` cannot reassemble a tag. */
const stripTags = (s: string) => {
  let prev: string;
  do {
    prev = s;
    s = s.replace(/<\/?[a-z_]+>/gi, '');
  } while (s !== prev);
  return s;
};

/** First readable line of a failed result, without the "Error:" / "exit=N" prefix. */
const failureLine = (t: ToolCallTrace) => {
  const r = (t.result ?? '').replace(/^exit=-?\d+\s*/, '').replace(/^error:?\s*/i, '');
  return firstLine(stripTags(r)) || 'Step failed';
};

/** Files a turn successfully modified (deduplicated, original path spelling kept). */
export function editedPaths(tools: ToolCallTrace[] | undefined): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const t of tools ?? []) {
    const v = describeTool(t);
    if (!v.edits || !v.path || t.result === undefined || isFailed(t)) continue;
    const key = normPath(v.path);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(v.path);
  }
  return out;
}

// ---------------------------------------------------------------------------------------------
// Small pieces
// ---------------------------------------------------------------------------------------------

export const DiffStat: React.FC<{ adds: number; dels: number }> = ({ adds, dels }) => (
  <span className="shrink-0 font-mono text-[11px] tabular-nums">
    <span className="text-[var(--added-fg)]">+{adds}</span>{' '}
    <span className="text-[var(--removed-fg)]">-{dels}</span>
  </span>
);

const CopyButton: React.FC<{ text: string }> = ({ text }) => {
  const [copied, setCopied] = useState(false);
  return (
    <button
      onClick={(e) => {
        e.stopPropagation();
        void navigator.clipboard?.writeText(text).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1200);
        });
      }}
      className="flex items-center justify-center w-5 h-5 rounded text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
      title={copied ? 'Copied' : 'Copy'}
    >
      {copied ? <Check size={11} /> : <Copy size={11} />}
    </button>
  );
};

/** Ticks once per second on its own so the rest of the panel never re-renders for a clock. */
export const Elapsed: React.FC<{ since: number; className?: string }> = ({ since, className }) => {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    return visibleInterval(() => setNow(Date.now()), 1000);
  }, []);
  return <span className={`tabular-nums ${className ?? ''}`}>{formatDur(Math.max(0, now - since)).replace(/\.\ds$/, 's')}</span>;
};

const StatusIcon: React.FC<{ status: ToolStatus; Icon: IconT }> = ({ status, Icon }) => {
  switch (status) {
    case 'running':
      return <Loader2 size={13} className="animate-spin text-[var(--accent)]" />;
    case 'failed':
      return <AlertCircle size={13} className="text-[var(--danger)]" />;
    case 'blocked':
      return <ShieldAlert size={13} className="text-[var(--modified-fg)]" />;
    case 'stopped':
      return <Icon size={13} className="text-[var(--text-dim)] opacity-50" />;
    default:
      return <Icon size={13} className="text-[var(--text-dim)]" />;
  }
};

const KIND_SUMMARY: Record<ToolKind, [string, string]> = {
  read: ['read {n} file', 'read {n} files'],
  search: ['searched {n} time', 'searched {n} times'],
  edit: ['edited {n} file', 'edited {n} files'],
  command: ['ran {n} command', 'ran {n} commands'],
  web: ['browsed {n} page', 'browsed {n} pages'],
  plan: ['updated the plan', 'updated the plan'],
  task: ['delegated {n} task', 'delegated {n} tasks'],
  other: ['used {n} tool', 'used {n} tools'],
};
const KIND_ORDER: ToolKind[] = ['edit', 'read', 'search', 'command', 'web', 'task', 'plan', 'other'];

function summarize(views: ToolView[]): string {
  const counts = new Map<ToolKind, number>();
  for (const v of views) counts.set(v.kind, (counts.get(v.kind) ?? 0) + 1);
  const parts = KIND_ORDER.filter((k) => counts.has(k)).map((k) => {
    const n = counts.get(k)!;
    return KIND_SUMMARY[k][n === 1 ? 0 : 1].replace('{n}', String(n));
  });
  const s = parts.slice(0, 3).join(', ') + (parts.length > 3 ? ', …' : '');
  return s[0].toUpperCase() + s.slice(1);
}

// ---------------------------------------------------------------------------------------------
// Step row
// ---------------------------------------------------------------------------------------------

const ToolStep: React.FC<{
  tool: ToolCallTrace;
  turnActive: boolean;
  open: boolean;
  onToggle: () => void;
  onOpenDiff: (path: string) => void;
  changeFor: ChangeLookup;
  /** Position in a group's tree: draws the connector. Omitted for a standalone step. */
  tree?: 'mid' | 'last';
}> = ({ tool, turnActive, open, onToggle, onOpenDiff, changeFor, tree }) => {
  const v = describeTool(tool);
  const status = toolStatus(tool, turnActive);
  const bad = status === 'failed' || status === 'blocked';
  const isCmd = v.kind === 'command';
  const body = v.detail || tool.result || '';
  const hasDetail = isCmd || !!body;
  const change = v.edits && v.path ? changeFor(v.path) : undefined;
  const opensDiff = v.edits && !!v.path && status === 'ok';
  const dur =
    tool.startedAt !== undefined && tool.endedAt !== undefined ? tool.endedAt - tool.startedAt : undefined;
  const exit = EXIT_RE.exec(tool.result?.trim() ?? '');
  const output = isCmd ? (tool.result ?? '').replace(EXIT_RE, '').replace(/^\r?\n/, '') : body;

  return (
    <div className={`relative ${tree ? 'pl-[16px]' : ''}`}>
      {tree && (
        <>
          <span
            aria-hidden
            className={`absolute left-0 top-0 w-px bg-[var(--border)] ${tree === 'last' ? 'h-[13px]' : 'h-full'}`}
          />
          <span aria-hidden className="absolute left-0 top-[13px] w-[11px] h-px bg-[var(--border)]" />
        </>
      )}
      <div
        onClick={() => (opensDiff ? onOpenDiff(v.path!) : hasDetail && onToggle())}
        className={`group relative flex items-center gap-2 h-[26px] pl-1 pr-1 rounded-md text-[12px] ${
          opensDiff || hasDetail ? 'cursor-pointer hover:bg-[var(--bg-hover)]' : ''
        }`}
        title={opensDiff ? `Open diff · ${v.title ?? v.subject}` : v.title ?? v.subject}
      >
        <span className="w-[14px] h-[14px] shrink-0 flex items-center justify-center">
          <StatusIcon status={status} Icon={v.Icon} />
        </span>
        <span className="flex items-center gap-1.5 min-w-0 flex-1">
          {v.intent ? (
            <span
              className={`min-w-0 truncate ${status === 'running' ? 'agent-shimmer' : 'text-[var(--text-muted)]'}`}
            >
              {v.intent}
            </span>
          ) : (
            <span className={`shrink-0 ${status === 'running' ? 'agent-shimmer' : 'text-[var(--text-muted)]'}`}>
              {status === 'running' ? v.active : v.verb}
            </span>
          )}
          {v.subject && (
            <span
              className={`px-1.5 py-px rounded bg-[var(--bg-elevated)] font-mono text-[11px] truncate ${
                v.intent ? 'shrink-0 max-w-[46%]' : 'min-w-0'
              } ${
                status === 'stopped' || v.intent ? 'text-[var(--text-dim)]' : 'text-[var(--text-primary)]'
              } ${opensDiff ? 'group-hover:text-[var(--accent)]' : ''}`}
            >
              {v.kind === 'search' && tool.name === 'search_code' ? `“${v.subject}”` : v.subject}
            </span>
          )}
          {v.meta && !change && (
            <span className="shrink-0 font-mono text-[10.5px] text-[var(--text-dim)] truncate max-w-[35%]">{v.meta}</span>
          )}
        </span>
        {change && <DiffStat adds={change.additions} dels={change.deletions} />}
        {status === 'blocked' && <span className="shrink-0 text-[10.5px] text-[var(--modified-fg)]">Blocked</span>}
        {status === 'failed' && <span className="shrink-0 text-[10.5px] text-[var(--danger)]">Failed</span>}
        {status === 'stopped' && <span className="shrink-0 text-[10.5px] text-[var(--text-dim)]">Stopped</span>}
        {status === 'ok' && dur !== undefined && dur >= 1000 && (
          <span className="shrink-0 text-[10.5px] tabular-nums text-[var(--text-dim)] opacity-0 group-hover:opacity-100 transition-opacity">
            {formatDur(dur)}
          </span>
        )}
        {status === 'running' && tool.startedAt !== undefined && (
          <Elapsed since={tool.startedAt} className="shrink-0 text-[10.5px] text-[var(--text-dim)]" />
        )}
        {hasDetail && !opensDiff && (
          <ChevronRight
            size={12}
            className={`shrink-0 text-[var(--text-dim)] transition-transform duration-150 ${open ? 'rotate-90' : ''} ${
              open ? '' : 'opacity-0 group-hover:opacity-100'
            }`}
          />
        )}
      </div>

      {bad && !open && (
        <div
          onClick={onToggle}
          className={`ml-[22px] mr-1 -mt-0.5 mb-0.5 text-[11px] leading-[16px] truncate cursor-pointer ${
            status === 'blocked' ? 'text-[var(--modified-fg)]/80' : 'text-[var(--danger)]/85'
          }`}
          title={tool.result}
        >
          {failureLine(tool)}
        </div>
      )}

      {open && hasDetail && (
        <div className="ml-[22px] mr-1 mt-0.5 mb-1.5 rounded-md border border-[var(--border)] bg-[var(--bg-panel)] overflow-hidden">
          {isCmd && (
            <div className="flex items-start gap-2 px-2.5 py-1.5 border-b border-[var(--border)] bg-[var(--bg-elevated)]/50">
              <span className="font-mono text-[11px] text-[var(--text-dim)] select-none">$</span>
              <span className="flex-1 min-w-0 font-mono text-[11px] text-[var(--text-primary)] whitespace-pre-wrap break-all">
                {v.title ?? v.subject}
              </span>
              {exit && (
                <span
                  className={`shrink-0 h-[18px] px-1.5 rounded font-mono text-[10px] leading-[18px] ${
                    exit[1] === '0'
                      ? 'text-[var(--added-fg)] bg-[var(--added-bg)]'
                      : 'text-[var(--removed-fg)] bg-[var(--removed-bg)]'
                  }`}
                >
                  exit {exit[1]}
                </span>
              )}
              <CopyButton text={v.title ?? v.subject} />
            </div>
          )}
          {output.trim() ? (
            <div className="relative group/out">
              <pre
                className={`m-0 px-2.5 py-2 text-[11px] leading-[1.55] font-mono max-h-56 overflow-auto whitespace-pre-wrap break-words ${
                  bad ? (status === 'blocked' ? 'text-[var(--modified-fg)]' : 'text-[var(--removed-fg)]') : 'text-[var(--text-muted)]'
                }`}
              >
                {output.length > 8000 ? `${output.slice(0, 8000)}\n… (truncated)` : output}
              </pre>
              {!isCmd && (
                <span className="absolute top-1 right-1 opacity-0 group-hover/out:opacity-100 transition-opacity">
                  <CopyButton text={output} />
                </span>
              )}
            </div>
          ) : (
            <div className="px-2.5 py-1.5 text-[11px] italic text-[var(--text-dim)]">
              {status === 'running' ? 'Waiting for output…' : 'No output'}
            </div>
          )}
        </div>
      )}
    </div>
  );
};

// ---------------------------------------------------------------------------------------------
// Group: one run of consecutive tool calls
// ---------------------------------------------------------------------------------------------

const GROUP_ICON: Record<ToolKind, IconT> = {
  read: FileText,
  search: Search,
  edit: SquarePen,
  command: TerminalIcon,
  web: Globe,
  plan: ListChecks,
  task: Bot,
  other: Wrench,
};

type GroupProps = {
  tools: ToolCallTrace[];
  turnActive: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  expanded: Record<string, boolean>;
  onToggleTool: (id: string) => void;
  onOpenDiff: (path: string) => void;
  changeFor: ChangeLookup;
};

/** A borderless, collapsible run of steps drawn as a tree. */
const GroupTree: React.FC<GroupProps> = ({
  tools,
  turnActive,
  open,
  onOpenChange,
  expanded,
  onToggleTool,
  onOpenDiff,
  changeFor,
}) => {
  const step = (t: ToolCallTrace, tree?: 'mid' | 'last') => (
    <ToolStep
      key={t.id}
      tool={t}
      turnActive={turnActive}
      open={!!expanded[t.id]}
      onToggle={() => onToggleTool(t.id)}
      onOpenDiff={onOpenDiff}
      changeFor={changeFor}
      tree={tree}
    />
  );

  // A lone step needs no group chrome.
  if (tools.length === 1) return <div className="py-0.5">{step(tools[0])}</div>;

  const statuses = tools.map((t) => toolStatus(t, turnActive));
  const views = tools.map(describeTool);
  const runningIdx = statuses.lastIndexOf('running');
  const running = runningIdx >= 0;
  const failed = statuses.filter((s) => s === 'failed').length;
  const blocked = statuses.filter((s) => s === 'blocked').length;
  const doneCount = statuses.filter((s) => s !== 'running').length;
  const starts = tools.map((t) => t.startedAt).filter((x): x is number => x !== undefined);
  const ends = tools.map((t) => t.endedAt).filter((x): x is number => x !== undefined);
  const span = !running && starts.length && ends.length ? Math.max(...ends) - Math.min(...starts) : undefined;
  const cur = running ? views[runningIdx] : undefined;
  const kinds = new Set(views.map((v) => v.kind));
  const GroupIcon = kinds.size === 1 ? GROUP_ICON[views[0].kind] : ListTree;
  const [verb, ...restWords] = summarize(views).split(' ');

  return (
    <div>
      <button
        onClick={() => onOpenChange(!open)}
        className="group w-full flex items-center gap-1.5 h-7 pl-1 pr-1.5 rounded-md text-left text-[12px] cursor-pointer hover:bg-[var(--bg-hover)]"
        aria-expanded={open}
      >
        <span className="w-[14px] h-[14px] shrink-0 flex items-center justify-center">
          {running ? (
            <Loader2 size={13} className="animate-spin text-[var(--accent)]" />
          ) : (
            <GroupIcon size={13} className="text-[var(--text-dim)]" />
          )}
        </span>
        <span className="min-w-0 truncate">
          {cur ? (
            <span className="agent-shimmer">{cur.intent || `${cur.active} ${cur.subject}`.trim()}</span>
          ) : (
            <>
              <span className="font-medium text-[var(--text-primary)]">{verb}</span>{' '}
              <span className="text-[var(--text-muted)]">{restWords.join(' ')}</span>
            </>
          )}
        </span>
        <ChevronDown
          size={12}
          className={`shrink-0 text-[var(--text-dim)] group-hover:text-[var(--text-muted)] transition-transform duration-150 ${
            open ? '' : '-rotate-90'
          }`}
        />
        <span className="flex-1" />
        {blocked > 0 && (
          <span className="shrink-0 flex items-center gap-1 text-[10.5px] text-[var(--modified-fg)]">
            <ShieldAlert size={11} />
            {blocked}
          </span>
        )}
        {failed > 0 && (
          <span className="shrink-0 flex items-center gap-1 text-[10.5px] text-[var(--danger)]">
            <AlertCircle size={11} />
            {failed}
          </span>
        )}
        {running && (
          <span className="shrink-0 text-[10.5px] tabular-nums text-[var(--text-dim)]">
            {doneCount}/{tools.length}
          </span>
        )}
        {span !== undefined && span >= 2000 && (
          <span className="shrink-0 text-[10.5px] tabular-nums text-[var(--text-dim)] opacity-0 group-hover:opacity-100 transition-opacity">
            {formatDur(span)}
          </span>
        )}
      </button>
      {open && (
        <div className="ml-[8px] py-0.5">
          {tools.map((t, i) => step(t, i === tools.length - 1 ? 'last' : 'mid'))}
        </div>
      )}
    </div>
  );
};

export const ActivityGroup: React.FC<GroupProps> = (props) => {
  const { tools, turnActive } = props;
  if (tools.length === 0) return null;

  // Steps still in flight at the tail sit below the (collapsible) completed group so it is obvious
  // where execution is right now without expanding past history.
  let split = tools.length;
  while (split > 0 && toolStatus(tools[split - 1], turnActive) === 'running') split--;
  if (split === 0 || split === tools.length) return <GroupTree {...props} />;

  return (
    <div>
      <GroupTree {...props} tools={tools.slice(0, split)} />
      <div className="py-0.5">
        {tools.slice(split).map((t) => (
          <ToolStep
            key={t.id}
            tool={t}
            turnActive={turnActive}
            open={!!props.expanded[t.id]}
            onToggle={() => props.onToggleTool(t.id)}
            onOpenDiff={props.onOpenDiff}
            changeFor={props.changeFor}
          />
        ))}
      </div>
    </div>
  );
};

/** Live "what is the agent doing" line at the foot of a streaming turn. */
export const LiveStatus: React.FC<{ label: string; since?: number }> = ({ label, since }) => (
  <div className="flex items-center gap-2 h-6 pl-[3px] text-[12px]">
    <span className="relative flex w-[14px] h-[14px] items-center justify-center">
      <span className="absolute w-2 h-2 rounded-full bg-[var(--accent)] opacity-40 animate-ping" />
      <span className="relative w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
    </span>
    <span className="agent-shimmer">{label}</span>
    {since !== undefined && <Elapsed since={since} className="text-[10.5px] text-[var(--text-dim)]" />}
  </div>
);
