import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import {
  AlertCircle,
  ArrowRight,
  ArrowUp,
  ArrowUpRight,
  AtSign,
  Bug,
  Check,
  ChevronDown,
  ChevronRight,
  FileCode,
  FolderOpen,
  GitBranch,
  GitCompare,
  History,
  Loader2,
  Plus,
  RotateCcw,
  Undo2,
  Search,
  Settings,
  Square,
  Terminal as TerminalIcon,
  Wrench,
  X,
  Zap,
} from 'lucide-react';
import { api, streamAgentChat } from '../api';
import type { LiteTheme } from '../themes';
import type {
  AgentConfig,
  CliToolStatus,
  Checkpoint,
  GitFileStatus,
  LocalModelsInfo,
  RollbackRes,
  ToastAction,
  ToolCallTrace,
  UiChatMessage,
} from '../types';
import { MarkdownMessage } from './MarkdownView';

const DEFAULT_CONFIG: AgentConfig = {
  provider: 'openai',
  baseUrl: '',
  apiKey: '',
  model: 'gpt-4.1',
  autoCheckpoint: true,
};

const PROVIDER_PRESETS: Record<AgentConfig['provider'], { model: string; baseUrl: string }> = {
  openai: { model: 'gpt-4.1', baseUrl: 'https://api.openai.com/v1' },
  anthropic: { model: 'claude-sonnet-4-20250514', baseUrl: 'https://api.anthropic.com' },
  gemini: { model: 'gemini-2.5-pro', baseUrl: '' },
  ollama: { model: 'qwen2.5-coder:14b', baseUrl: 'http://127.0.0.1:11434/v1' },
  'claude-cli': { model: 'default', baseUrl: '' },
  'codex-cli': { model: 'default', baseUrl: '' },
};

// Fallbacks only: Ollama and Codex lists are replaced by what is actually installed.
const MODEL_PRESETS: Record<AgentConfig['provider'], string[]> = {
  openai: ['gpt-4.1', 'gpt-4.1-mini', 'o4-mini'],
  anthropic: ['claude-sonnet-4-20250514', 'claude-opus-4-20250514'],
  gemini: ['gemini-2.5-pro', 'gemini-2.5-flash'],
  ollama: ['qwen2.5-coder:14b', 'llama3.1', 'deepseek-r1'],
  'claude-cli': ['default', 'sonnet', 'opus', 'haiku'],
  'codex-cli': ['default'],
};

const PROVIDER_LABEL: Record<AgentConfig['provider'], string> = {
  openai: 'OpenAI',
  anthropic: 'Anthropic',
  gemini: 'Gemini',
  ollama: 'Ollama',
  'claude-cli': 'Claude Code',
  'codex-cli': 'Codex',
};

/** Providers that run an installed CLI with its own login instead of an API key. */
const CLI_PROVIDER_TOOL: Partial<Record<AgentConfig['provider'], string>> = {
  'claude-cli': 'claude',
  'codex-cli': 'codex',
};

type Provider = AgentConfig['provider'];
const CLI_PROVIDERS: Provider[] = ['claude-cli', 'codex-cli'];
const API_PROVIDERS: Provider[] = ['anthropic', 'openai', 'gemini'];

const OK_DOT = '#10b981';
const WARN_DOT = '#f59e0b';

/** Close a popover on outside click / Escape. Clicks on `[data-pop="<trigger>"]` are ignored so the trigger can toggle. */
function usePopoverClose(open: boolean, onClose: () => void, trigger?: string) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const target = e.target as Element;
      if (trigger && target.closest?.(`[data-pop="${trigger}"]`)) return;
      if (ref.current && !ref.current.contains(target)) onClose();
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
  }, [open, onClose, trigger]);
  return ref;
}

const baseName = (p: string) => p.split(/[\\/]/).pop() || p;
const normPath = (p: string) => p.replace(/\\/g, '/').replace(/^\.\//, '');

const shortModel = (m: string) => (m === 'default' ? '' : m.replace(/-\d{8}$/, ''));

const formatDur = (ms: number) => `${Math.round(ms / 100) / 10}s`;

function timeAgo(ts: number): string {
  const s = (Date.now() - ts) / 1000;
  if (s < 60) return 'now';
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

// ---------------------------------------------------------------------------------------------
// Threads (persisted per workspace)
// ---------------------------------------------------------------------------------------------

interface AgentThread {
  id: string;
  title: string;
  updatedAt: number;
  messages: UiChatMessage[];
}

const MAX_THREADS = 20;
const threadsKey = (ws: string) => `vireo-agent-threads:${ws}`;
const newThreadId = () => `t-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;

function loadThreads(ws: string): AgentThread[] {
  try {
    const raw = localStorage.getItem(threadsKey(ws));
    const parsed = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function slimMessages(ms: UiChatMessage[]): UiChatMessage[] {
  return ms.map((m) => ({
    ...m,
    streaming: false,
    tools: m.tools?.map((t) => ({
      ...t,
      result: t.result && t.result.length > 2000 ? `${t.result.slice(0, 2000)}\n… (truncated)` : t.result,
    })),
  }));
}

const threadTitle = (ms: UiChatMessage[]) => {
  const first = ms.find((m) => m.role === 'user');
  return first ? first.content.replace(/\s+/g, ' ').trim().slice(0, 60) : 'New thread';
};

// ---------------------------------------------------------------------------------------------
// Tool trace
// ---------------------------------------------------------------------------------------------

interface ToolView {
  Icon: React.ComponentType<{ size?: number; className?: string }>;
  verb: string;
  subject: string;
  meta?: string;
  /** Workspace file this call touched (clicking opens it in the diff viewer). */
  path?: string;
  edits: boolean;
}

function describeTool(t: ToolCallTrace): ToolView {
  const str = (k: string) => (typeof t.args?.[k] === 'string' ? (t.args[k] as string) : '');
  const num = (k: string) => (typeof t.args?.[k] === 'number' ? (t.args[k] as number) : undefined);
  switch (t.name) {
    case 'read_file': {
      const s = num('start_line');
      const e = num('end_line');
      const path = str('path');
      return {
        Icon: FileCode,
        verb: 'Read',
        subject: path,
        meta: str('symbol') || (s || e ? `L${s ?? 1}–${e ?? ''}` : undefined),
        path,
        edits: false,
      };
    }
    case 'outline':
      return { Icon: FileCode, verb: 'Outline', subject: str('path'), path: str('path'), edits: false };
    case 'repo_map':
      return { Icon: FolderOpen, verb: 'Mapped', subject: str('query') || str('dir') || 'repo', edits: false };
    case 'symbol_edit': {
      const mode = str('mode');
      const mut = mode === 'replace' || mode === 'insert_after' || mode === 'delete';
      return { Icon: mut ? GitCompare : FileCode, verb: mut ? 'Edited' : 'Read', subject: str('path'), meta: str('symbol'), path: str('path'), edits: mut };
    }
    case 'ast':
      return { Icon: Search, verb: str('rewrite') ? 'Rewrote' : 'AST', subject: str('pattern'), edits: !!str('rewrite') };
    case 'verify':
      return { Icon: TerminalIcon, verb: 'Verified', subject: str('command') || 'build', edits: false };
    case 'search_code':
      return { Icon: Search, verb: 'Searched', subject: `"${str('query')}"`, edits: false };
    case 'list_files':
      return { Icon: FolderOpen, verb: 'Listed', subject: str('dir') || 'workspace root', edits: false };
    case 'edit_file':
      return { Icon: GitCompare, verb: 'Edited', subject: str('path'), path: str('path'), edits: true };
    case 'write_file':
      return { Icon: GitCompare, verb: 'Wrote', subject: str('path'), path: str('path'), edits: true };
    case 'run_command':
      return { Icon: TerminalIcon, verb: 'Ran', subject: str('command'), edits: false };
    default:
      return { Icon: Wrench, verb: t.name.replace(/_/g, ' '), subject: str('path'), edits: false };
  }
}

const isFailed = (t: ToolCallTrace) => t.result !== undefined && /^error/i.test(t.result.trim());

/** Files a turn successfully modified (deduplicated, original path spelling kept). */
function editedPaths(tools: ToolCallTrace[] | undefined): string[] {
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

type ChangeLookup = (path: string) => GitFileStatus | undefined;

const DiffStat: React.FC<{ adds: number; dels: number }> = ({ adds, dels }) => (
  <span className="shrink-0 font-mono text-[11px] tabular-nums">
    <span className="text-[var(--added-fg)]">+{adds}</span>{' '}
    <span className="text-[var(--removed-fg)]">-{dels}</span>
  </span>
);

const ToolRow: React.FC<{
  tool: ToolCallTrace;
  turnActive: boolean;
  open: boolean;
  onToggle: () => void;
  onOpenDiff: (path: string) => void;
  changeFor: ChangeLookup;
}> = ({ tool, turnActive, open, onToggle, onOpenDiff, changeFor }) => {
  const v = describeTool(tool);
  const done = tool.result !== undefined;
  const failed = isFailed(tool);
  const detail =
    tool.name === 'run_command'
      ? `$ ${v.subject}${tool.result ? `\n\n${tool.result}` : ''}`
      : tool.result || '';
  const change = v.edits && v.path ? changeFor(v.path) : undefined;
  const opensDiff = v.edits && !!v.path && done && !failed;
  return (
    <div>
      <div
        onClick={() => (opensDiff ? onOpenDiff(v.path!) : detail && onToggle())}
        className="group flex items-center gap-1.5 h-6 pr-1 rounded cursor-pointer hover:bg-[var(--bg-hover)] text-[12px] text-[var(--text-muted)]"
        title={opensDiff ? 'Open diff' : v.subject}
      >
        <span className="w-3 shrink-0 flex items-center justify-center">
          {!done && turnActive ? (
            <Loader2 size={12} className="animate-spin text-[var(--accent)]" />
          ) : failed ? (
            <AlertCircle size={12} className="text-[var(--danger)]" />
          ) : done ? (
            <Check size={12} className="text-[var(--added-fg)]" />
          ) : (
            <v.Icon size={12} className="text-[var(--text-dim)]" />
          )}
        </span>
        <span className="w-[52px] shrink-0 truncate">{v.verb}</span>
        <span className="font-mono text-[11.5px] truncate min-w-0 text-[var(--text-primary)]/90" title={v.subject}>
          {v.path ? baseName(v.subject) : v.subject}
        </span>
        <span className="flex-1" />
        {change ? (
          <DiffStat adds={change.additions} dels={change.deletions} />
        ) : (
          v.meta && <span className="shrink-0 font-mono text-[11px] text-[var(--text-dim)] truncate max-w-[40%]">{v.meta}</span>
        )}
        {detail && (
          <button
            onClick={(e) => {
              e.stopPropagation();
              onToggle();
            }}
            className="shrink-0 text-[var(--text-dim)] hover:text-[var(--text-primary)] opacity-0 group-hover:opacity-100 cursor-pointer"
            title="Show output"
          >
            {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          </button>
        )}
      </div>
      {open && detail && (
        <pre className="my-1 ml-5 p-2 rounded bg-[var(--bg-panel)] border border-[var(--border)]/70 text-[11px] font-mono text-[var(--text-muted)] max-h-40 overflow-auto whitespace-pre-wrap break-words">
          {detail.length > 4000 ? `${detail.slice(0, 4000)}\n… (truncated)` : detail}
        </pre>
      )}
    </div>
  );
};

const ToolGroup: React.FC<{
  tools: ToolCallTrace[];
  turnActive: boolean;
  durationMs?: number;
  expanded: Record<string, boolean>;
  onToggle: (id: string) => void;
  onOpenDiff: (path: string) => void;
  changeFor: ChangeLookup;
}> = ({ tools, turnActive, durationMs, expanded, onToggle, onOpenDiff, changeFor }) => {
  const doneCount = tools.filter((t) => t.result !== undefined).length;
  const working = turnActive && doneCount < tools.length;
  return (
    <div className="border-l-2 border-[var(--border)] pl-2.5 my-1.5">
      <div className="flex items-center justify-between h-5 text-[11px] text-[var(--text-dim)]">
        <span>
          {working
            ? `Working · ${doneCount}/${tools.length} steps`
            : `${tools.length} step${tools.length === 1 ? '' : 's'} completed`}
        </span>
        {!turnActive && durationMs !== undefined && durationMs >= 100 && <span>{formatDur(durationMs)}</span>}
      </div>
      {tools.map((t) => (
        <ToolRow
          key={t.id}
          tool={t}
          turnActive={turnActive}
          open={!!expanded[t.id]}
          onToggle={() => onToggle(t.id)}
          onOpenDiff={onOpenDiff}
          changeFor={changeFor}
        />
      ))}
    </div>
  );
};

const TouchedFiles: React.FC<{
  paths: string[];
  changeFor: ChangeLookup;
  onOpenDiff: (path: string) => void;
}> = ({ paths, changeFor, onOpenDiff }) => (
  <div className="rounded-md border border-[var(--border)] bg-[var(--bg-elevated)]/60 overflow-hidden">
    {paths.map((p, i) => {
      const c = changeFor(p);
      return (
        <div
          key={p}
          className={`flex items-center gap-2 h-[28px] px-2.5 text-[12px] ${i > 0 ? 'border-t border-[var(--border)]/70' : ''}`}
        >
          <FileCode size={12} className="shrink-0 text-[var(--text-muted)]" />
          <span className="font-mono text-[11.5px] truncate min-w-0 flex-1 text-[var(--text-primary)]" title={p}>
            {baseName(p)}
          </span>
          {c && <DiffStat adds={c.additions} dels={c.deletions} />}
          <button
            onClick={() => onOpenDiff(p)}
            className="shrink-0 text-[11px] text-[var(--accent)] hover:underline cursor-pointer"
          >
            View diff ↗
          </button>
        </div>
      );
    })}
  </div>
);

const ContextBadge: React.FC<{ icon: React.ReactNode; label: string; title?: string }> = ({ icon, label, title }) => (
  <span
    className="inline-flex items-center gap-1 h-[18px] px-1.5 rounded bg-[var(--bg-hover)] text-[11px] text-[var(--text-muted)] max-w-full"
    title={title}
  >
    {icon}
    <span className="truncate">{label}</span>
  </span>
);

const plural = (n: number) => `${n} file${n === 1 ? '' : 's'}`;

const CheckpointDivider: React.FC<{
  checkpoint: Checkpoint;
  onUndo: () => void;
  onRedo: () => void;
  onForce: () => void;
}> = ({ checkpoint, onUndo, onRedo, onForce }) => {
  const total = checkpoint.files?.length ?? 0;
  const reverted = checkpoint.redo_files?.length ?? 0;
  const pending = Math.max(0, total - reverted);
  const adds = checkpoint.files?.reduce((n, f) => n + f.additions, 0) ?? 0;
  const dels = checkpoint.files?.reduce((n, f) => n + f.deletions, 0) ?? 0;
  const btn =
    'shrink-0 px-1.5 py-0.5 rounded text-[var(--accent)] hover:bg-[var(--bg-hover)] cursor-pointer';
  return (
    <div className="flex items-center gap-2 my-1 text-[11px] text-[var(--text-dim)]">
      <div className="flex-1 h-px bg-[var(--border)]" />
      <span
        className="flex items-center gap-1 min-w-0 truncate"
        title={checkpoint.files?.map((f) => f.path).join('\n')}
      >
        {reverted > 0 ? <Undo2 size={10} className="shrink-0" /> : <RotateCcw size={10} className="shrink-0" />}
        {reverted === 0 ? (
          <>
            Checkpoint · {plural(total)}{' '}
            <span className="font-mono tabular-nums">
              (<span className="text-[var(--added-fg)]">+{adds}</span>{' '}
              <span className="text-[var(--removed-fg)]">-{dels}</span>)
            </span>
          </>
        ) : pending === 0 ? (
          <>Undone · {plural(reverted)} restored</>
        ) : (
          <>
            Undone {reverted} · {pending} changed since
          </>
        )}
      </span>
      {reverted === 0 && (
        <button onClick={onUndo} className={btn}>
          Undo turn
        </button>
      )}
      {pending > 0 && reverted > 0 && (
        <button
          onClick={onForce}
          className={btn}
          title="Overwrite the files that changed after this turn. Their current content stays redoable."
        >
          Force revert {pending}
        </button>
      )}
      {reverted > 0 && (
        <button onClick={onRedo} className={btn}>
          Redo turn
        </button>
      )}
      <div className="w-4 h-px bg-[var(--border)]" />
    </div>
  );
};

// ---------------------------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------------------------

interface AgentPanelProps {
  theme: LiteTheme;
  workspacePath: string;
  draftAppend: { id: number; text: string } | null;
  /** Active editor/diff file, offered as a context chip. */
  activeFilePath: string | null;
  changedFileCount: number;
  changedFiles: GitFileStatus[];
  branch: string;
  allFiles: string[];
  onLaunchCliAgent: () => void;
  onOpenDiff: (path: string) => void;
  onRefreshGit: () => Promise<void>;
  onToast: (msg: string, isError?: boolean, action?: ToastAction) => void;
}

interface AtRange {
  start: number;
  end: number;
  query: string;
}

export const AgentPanel: React.FC<AgentPanelProps> = ({
  theme,
  workspacePath,
  draftAppend,
  activeFilePath,
  changedFileCount,
  changedFiles,
  branch,
  allFiles,
  onLaunchCliAgent,
  onOpenDiff,
  onRefreshGit,
  onToast,
}) => {
  const [showSettings, setShowSettings] = useState(false);
  const [showModelMenu, setShowModelMenu] = useState(false);
  const [showThreads, setShowThreads] = useState(false);
  const [showFilePicker, setShowFilePicker] = useState(false);
  const [fileQuery, setFileQuery] = useState('');
  const [atRange, setAtRange] = useState<AtRange | null>(null);
  const [pickIdx, setPickIdx] = useState(0);
  const [customModel, setCustomModel] = useState('');

  const [config, setConfig] = useState<AgentConfig>(() => {
    try {
      const saved = localStorage.getItem('vireo-agent-config');
      if (saved) return { ...DEFAULT_CONFIG, ...JSON.parse(saved) };
    } catch {
      // ignore
    }
    return DEFAULT_CONFIG;
  });
  // Keys are remembered per provider so switching engines never leaks one provider's key to another.
  const [apiKeys, setApiKeys] = useState<Record<string, string>>(() => {
    try {
      return JSON.parse(localStorage.getItem('vireo-agent-keys') || '{}');
    } catch {
      return {};
    }
  });

  const [messages, setMessages] = useState<UiChatMessage[]>([]);
  const [threadId, setThreadId] = useState(newThreadId);
  const [threadStore, setThreadStore] = useState(() => ({
    ws: workspacePath,
    list: loadThreads(workspacePath),
  }));
  const [input, setInput] = useState('');
  const [running, setRunning] = useState(false);
  const [expandedTools, setExpandedTools] = useState<Record<string, boolean>>({});
  // Context chips: the active file is on by default (per-path dismiss); changes are opt-in.
  const [dismissedActive, setDismissedActive] = useState<string | null>(null);
  const [includeChanges, setIncludeChanges] = useState(false);
  const [extraFiles, setExtraFiles] = useState<string[]>([]);
  const abortRef = useRef<AbortController | null>(null);
  const endRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const settingsRef = usePopoverClose(showSettings, () => setShowSettings(false), 'settings');
  const modelRef = usePopoverClose(showModelMenu, () => setShowModelMenu(false), 'engine');
  const threadsRef = usePopoverClose(showThreads, () => setShowThreads(false), 'threads');
  const pickerOpen = showFilePicker || atRange !== null;
  const pickerRef = usePopoverClose(
    pickerOpen,
    () => {
      setShowFilePicker(false);
      setAtRange(null);
    },
    'picker'
  );

  const [cliTools, setCliTools] = useState<CliToolStatus[]>([]);
  const [localModels, setLocalModels] = useState<LocalModelsInfo | null>(null);

  // Refresh what is installed locally (CLIs, Ollama models) at mount and whenever a menu opens.
  useEffect(() => {
    let cancelled = false;
    api
      .cliTools()
      .then((t) => !cancelled && setCliTools(t))
      .catch(() => {});
    api
      .agentModels(config.provider === 'ollama' ? config.baseUrl : undefined)
      .then((m) => !cancelled && setLocalModels(m))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showSettings, showModelMenu, config.provider]);

  const cliToolId = CLI_PROVIDER_TOOL[config.provider];
  const cliInstalled = cliToolId ? cliTools.find((t) => t.id === cliToolId)?.installed : undefined;
  const configured = cliToolId
    ? cliInstalled !== false
    : config.provider === 'ollama' || !!config.apiKey.trim();
  const ollamaModels = localModels?.ollama.running ? localModels.ollama.models : [];
  const modelOptions: string[] =
    config.provider === 'ollama' && ollamaModels.length
      ? ollamaModels
      : config.provider === 'codex-cli' && localModels?.codex.length
        ? ['default', ...localModels.codex]
        : MODEL_PRESETS[config.provider];
  const activeChip =
    activeFilePath && activeFilePath !== dismissedActive && !extraFiles.includes(activeFilePath)
      ? activeFilePath
      : null;

  // Engine readiness, shown as a dot in the composer and as the status card in the empty state.
  const engineName = cliToolId
    ? `${PROVIDER_LABEL[config.provider]} CLI`
    : config.provider === 'ollama'
      ? 'Ollama Local'
      : `${PROVIDER_LABEL[config.provider]} API`;
  const engineStatus: { ok: boolean; text: string } = cliToolId
    ? cliInstalled === false
      ? { ok: false, text: 'Not found' }
      : { ok: true, text: 'Connected' }
    : config.provider === 'ollama'
      ? localModels && !localModels.ollama.running
        ? { ok: false, text: 'Not running' }
        : { ok: true, text: ollamaModels.length ? `${ollamaModels.length} models ready` : 'Connected' }
      : config.apiKey.trim()
        ? { ok: true, text: 'Key set' }
        : { ok: false, text: 'No API key' };
  const engineShort = `${PROVIDER_LABEL[config.provider]}${shortModel(config.model) ? ` · ${shortModel(config.model)}` : ''}`;

  useEffect(() => {
    try {
      localStorage.setItem('vireo-agent-config', JSON.stringify(config));
    } catch {
      // ignore
    }
  }, [config]);

  useEffect(() => {
    setApiKeys((prev) =>
      prev[config.provider] === config.apiKey ? prev : { ...prev, [config.provider]: config.apiKey }
    );
  }, [config.provider, config.apiKey]);

  useEffect(() => {
    try {
      localStorage.setItem('vireo-agent-keys', JSON.stringify(apiKeys));
    } catch {
      // ignore
    }
  }, [apiKeys]);

  // Persist threads per workspace; a workspace switch swaps the list and starts a fresh thread.
  useEffect(() => {
    try {
      localStorage.setItem(threadsKey(threadStore.ws), JSON.stringify(threadStore.list));
    } catch {
      // ignore
    }
  }, [threadStore]);

  useEffect(() => {
    if (workspacePath === threadStore.ws) return;
    abortRef.current?.abort();
    setRunning(false);
    setMessages([]);
    setExpandedTools({});
    setThreadId(newThreadId());
    setThreadStore({ ws: workspacePath, list: loadThreads(workspacePath) });
  }, [workspacePath, threadStore.ws]);

  useEffect(() => {
    if (running || messages.length === 0) return;
    setThreadStore((prev) => ({
      ws: prev.ws,
      list: [
        { id: threadId, title: threadTitle(messages), updatedAt: Date.now(), messages: slimMessages(messages) },
        ...prev.list.filter((t) => t.id !== threadId),
      ].slice(0, MAX_THREADS),
    }));
  }, [messages, running, threadId]);

  useEffect(() => {
    if (draftAppend?.text) {
      setInput((prev) => (prev ? `${prev}\n\n${draftAppend.text}` : draftAppend.text));
    }
  }, [draftAppend]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  // Auto-growing composer: 44px up to 180px.
  useLayoutEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 180)}px`;
  }, [input]);

  const pickerQuery = atRange ? atRange.query : fileQuery;
  useEffect(() => setPickIdx(0), [pickerQuery]);

  const changeFor: ChangeLookup = useMemo(() => {
    return (p: string) => {
      const n = normPath(p);
      return changedFiles.find((c) => {
        const cp = normPath(c.path);
        return n === cp || n.endsWith(`/${cp}`) || cp.endsWith(`/${n}`);
      });
    };
  }, [changedFiles]);

  // Files this thread edited that are still uncommitted/unreverted.
  const liveEdits = useMemo(() => {
    const seen = new Set<string>();
    const out: { path: string; change: GitFileStatus }[] = [];
    for (const m of messages) {
      for (const p of editedPaths(m.tools)) {
        const change = changeFor(p);
        const key = normPath(change?.path ?? p);
        if (!change || seen.has(key)) continue;
        seen.add(key);
        out.push({ path: p, change });
      }
    }
    return out;
  }, [messages, changeFor]);
  const editTotals = liveEdits.reduce(
    (acc, e) => ({ adds: acc.adds + e.change.additions, dels: acc.dels + e.change.deletions }),
    { adds: 0, dels: 0 }
  );
  const lastCheckpoint = useMemo(
    () => [...messages].reverse().find((m) => m.checkpoint)?.checkpoint,
    [messages]
  );

  const setProvider = (p: Provider) =>
    setConfig((prev) => ({
      ...prev,
      provider: p,
      model:
        p === 'ollama' && ollamaModels.length ? ollamaModels[0] : PROVIDER_PRESETS[p].model,
      baseUrl: PROVIDER_PRESETS[p].baseUrl,
      apiKey: apiKeys[p] ?? '',
    }));

  const buildContextPrefix = (): string => {
    const files = [...(activeChip ? [activeChip] : []), ...extraFiles];
    const parts: string[] = [];
    if (files.length) parts.push(`Files in focus: ${files.join(', ')}.`);
    if (includeChanges && changedFileCount > 0)
      parts.push(`The workspace has ${changedFileCount} uncommitted changed file(s) (see git status/diff).`);
    return parts.length
      ? `[Context from the editor — ${parts.join(' ')} Use read_file to inspect them if relevant.]\n\n`
      : '';
  };

  const patchAssistant = (id: string, fn: (m: UiChatMessage) => UiChatMessage) =>
    setMessages((prev) => prev.map((m) => (m.id === id ? fn(m) : m)));

  const sendPrompt = async () => {
    const trimmed = input.trim();
    if (!trimmed || running) return;

    const now = Date.now();
    const userMsg: UiChatMessage = {
      id: `u-${now}`,
      role: 'user',
      content: trimmed,
      ctxFiles: [...(activeChip ? [activeChip] : []), ...extraFiles],
      ctxChanges: includeChanges && changedFileCount > 0 ? changedFileCount : undefined,
    };
    const assistantId = `a-${now + 1}`;
    const assistantMsg: UiChatMessage = {
      id: assistantId,
      role: 'assistant',
      content: '',
      tools: [],
      streaming: true,
      startedAt: now,
    };

    const historyPayload = [
      ...messages.map((m) => ({ role: m.role, content: m.content })),
      { role: 'user', content: buildContextPrefix() + trimmed },
    ];

    setMessages((prev) => [...prev, userMsg, assistantMsg]);
    setInput('');
    setAtRange(null);
    setRunning(true);

    const ctrl = new AbortController();
    abortRef.current = ctrl;

    try {
      await streamAgentChat(
        config,
        historyPayload,
        {
          // The backend emits one `text` event per model turn (not token deltas), so
          // consecutive events are separate paragraphs.
          onText: (text) =>
            patchAssistant(assistantId, (m) => ({
              ...m,
              content: m.content ? `${m.content}\n\n${text}` : text,
              firstOutputAt: m.firstOutputAt ?? Date.now(),
            })),
          onToolCall: (call) =>
            patchAssistant(assistantId, (m) => ({
              ...m,
              tools: [...(m.tools || []), { id: call.id, name: call.name, args: call.args }],
              firstOutputAt: m.firstOutputAt ?? Date.now(),
            })),
          onToolResult: (res) => {
            patchAssistant(assistantId, (m) => ({
              ...m,
              tools: (m.tools || []).map((t) => (t.id === res.id ? { ...t, result: res.result } : t)),
            }));
            if (['write_file', 'edit_file', 'symbol_edit', 'ast'].includes(res.name)) {
              onRefreshGit();
            }
          },
          onCheckpoint: (cp) => {
            patchAssistant(assistantId, (m) => ({ ...m, checkpoint: cp }));
            onRefreshGit();
          },
          onError: (msg) => patchAssistant(assistantId, (m) => ({ ...m, error: msg })),
          onDone: () => {
            patchAssistant(assistantId, (m) => ({ ...m, streaming: false, endedAt: Date.now() }));
            setRunning(false);
            onRefreshGit();
          },
        },
        ctrl.signal
      );
    } catch (e: unknown) {
      if ((e as Error).name !== 'AbortError') {
        patchAssistant(assistantId, (m) => ({ ...m, error: String(e), streaming: false, endedAt: Date.now() }));
      }
      setRunning(false);
    }
  };

  const stopGeneration = () => {
    abortRef.current?.abort();
    setRunning(false);
    setMessages((prev) =>
      prev.map((m) => (m.streaming ? { ...m, streaming: false, endedAt: Date.now() } : m))
    );
  };

  const newThread = () => {
    abortRef.current?.abort();
    setRunning(false);
    setMessages([]);
    setExpandedTools({});
    setThreadId(newThreadId());
    setShowThreads(false);
  };

  const openThread = (t: AgentThread) => {
    abortRef.current?.abort();
    setRunning(false);
    setMessages(t.messages);
    setExpandedTools({});
    setThreadId(t.id);
    setShowThreads(false);
  };

  const removeThread = (id: string) => {
    setThreadStore((prev) => ({ ws: prev.ws, list: prev.list.filter((t) => t.id !== id) }));
    if (id === threadId) {
      abortRef.current?.abort();
      setRunning(false);
      setMessages([]);
      setThreadId(newThreadId());
    }
  };

  const prefill = (text: string) => {
    setInput(text);
    requestAnimationFrame(() => textareaRef.current?.focus());
  };

  const quickActions: { icon: React.ReactNode; label: string; run: () => void; external?: boolean }[] = [];
  if (changedFileCount > 0) {
    quickActions.push({
      icon: <GitBranch size={13} />,
      label: `Review ${changedFileCount} uncommitted change${changedFileCount === 1 ? '' : 's'}`,
      run: () => {
        setIncludeChanges(true);
        prefill('Review my uncommitted changes. Run git diff, then point out bugs, risky edits and missing edge cases.');
      },
    });
  }
  if (activeFilePath) {
    quickActions.push({
      icon: <FileCode size={13} />,
      label: `Explain ${baseName(activeFilePath)}`,
      run: () =>
        prefill(`Explain what ${activeFilePath} does and how it fits into the rest of the project.`),
    });
    if (changedFileCount > 0) {
      quickActions.push({
        icon: <Bug size={13} />,
        label: 'Find bugs & edge cases in current diff',
        run: () =>
          prefill(`Look for bugs and edge cases in the uncommitted changes to ${activeFilePath}.`),
      });
    }
  } else {
    quickActions.push({
      icon: <FolderOpen size={13} />,
      label: 'Explain this project',
      run: () => prefill('Give me a short map of this project: main modules, entry points and how they fit together.'),
    });
  }
  quickActions.push({
    icon: <TerminalIcon size={13} />,
    label: 'Open interactive CLI in terminal dock',
    run: onLaunchCliAgent,
    external: true,
  });

  const filteredFiles = (() => {
    const q = pickerQuery.trim().toLowerCase();
    const pool = allFiles.filter((f) => !extraFiles.includes(f));
    return (q ? pool.filter((f) => f.toLowerCase().includes(q)) : pool).slice(0, 40);
  })();

  const addContextFile = (f: string) => {
    setExtraFiles((prev) => (prev.includes(f) ? prev : [...prev, f]));
    if (atRange) setInput((prev) => prev.slice(0, atRange.start) + prev.slice(atRange.end));
    setAtRange(null);
    setShowFilePicker(false);
    requestAnimationFrame(() => textareaRef.current?.focus());
  };

  const applyRollback = (res: RollbackRes) => {
    const next = res.checkpoint;
    if (next) setMessages((prev) => prev.map((m) => (m.checkpoint?.id === next.id ? { ...m, checkpoint: next } : m)));
    onRefreshGit();
  };

  const redoCheckpoint = async (id: string) => {
    try {
      const res = await api.gitRedo(id);
      applyRollback(res);
      if (res.ok) onToast(`Re-applied ${plural(res.restored)}`);
      else onToast(res.stderr || 'Redo failed', true);
    } catch (e) {
      onToast(e instanceof Error ? e.message : 'Redo failed', true);
    }
  };

  const undoCheckpoint = async (id: string, force = false) => {
    try {
      const res = await api.gitRollback(id, force);
      applyRollback(res);
      const redo = res.restored > 0 ? { label: 'Redo', run: () => void redoCheckpoint(id) } : undefined;
      if (res.conflicts.length > 0) {
        onToast(
          `${plural(res.conflicts.length)} changed since this turn and ${res.conflicts.length === 1 ? 'was' : 'were'} left alone` +
            (res.restored > 0 ? ` · reverted ${res.restored}` : ''),
          true,
          redo
        );
      } else if (res.ok) {
        onToast(res.restored > 0 ? `Reverted ${plural(res.restored)}` : 'Nothing to revert', false, redo);
      } else {
        onToast(res.stderr || 'Rollback failed', true);
      }
    } catch (e) {
      onToast(e instanceof Error ? e.message : 'Rollback failed', true);
    }
  };

  const chooseCli = (p: Provider) => {
    const toolId = CLI_PROVIDER_TOOL[p];
    const tool = cliTools.find((t) => t.id === toolId);
    if (tool && !tool.installed) {
      onToast(`${PROVIDER_LABEL[p]} CLI not found. ${tool.install_hint}`, true);
      return;
    }
    setProvider(p);
    setShowModelMenu(false);
  };

  const chooseApi = (p: Provider) => {
    setProvider(p);
    setShowModelMenu(false);
    if (!(apiKeys[p] ?? '').trim()) setShowSettings(true);
  };

  const chooseOllamaModel = (m: string) => {
    setConfig((prev) => ({
      ...prev,
      provider: 'ollama',
      model: m,
      baseUrl: prev.provider === 'ollama' && prev.baseUrl ? prev.baseUrl : PROVIDER_PRESETS.ollama.baseUrl,
      apiKey: '',
    }));
    setShowModelMenu(false);
  };

  const inputCls =
    'w-full px-2 py-1 rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none focus:border-[var(--accent)]';
  const iconBtn =
    'w-[26px] h-[26px] flex items-center justify-center rounded-md text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors';
  const menuHead = 'px-3 pt-1.5 pb-1 text-[10.5px] uppercase tracking-wider text-[var(--text-dim)]';
  const menuRow =
    'flex items-center gap-2 h-[26px] px-3 cursor-pointer hover:bg-[var(--bg-hover)] text-[var(--text-primary)]';

  const threadName = messages.length ? threadTitle(messages) : 'New thread';
  const workspaceName = baseName(workspacePath.replace(/[\\/]+$/, '')) || workspacePath;

  return (
    <div className="relative flex flex-col h-full min-w-0 overflow-hidden bg-[var(--bg-sidebar)] border-l border-[var(--border)]">
      {/* Zone 1: header */}
      <div className="flex items-center justify-between gap-1 px-3 h-9 border-b border-[var(--border)] shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          <span className="text-[11px] font-semibold tracking-wider uppercase text-[var(--text-muted)] shrink-0">
            Agent
          </span>
          <span className="w-px h-3.5 bg-[var(--border)] shrink-0" />
          <button
            data-pop="threads"
            onClick={() => setShowThreads((v) => !v)}
            className="flex items-center gap-1 min-w-0 max-w-[180px] h-[26px] px-1.5 -mx-1 rounded-md text-[12px] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
            title={threadName}
          >
            <span className="truncate">{threadName}</span>
            <ChevronDown size={11} className="shrink-0 text-[var(--text-dim)]" />
          </button>
        </div>
        <div className="flex items-center gap-0.5 shrink-0">
          <button onClick={newThread} className={iconBtn} title="New thread">
            <Plus size={15} />
          </button>
          <button
            data-pop="threads"
            onClick={() => setShowThreads((v) => !v)}
            className={`${iconBtn} ${showThreads ? '!text-[var(--accent)]' : ''}`}
            title="Recent threads"
          >
            <History size={14} />
          </button>
          <button
            data-pop="settings"
            onClick={() => setShowSettings((v) => !v)}
            className={`${iconBtn} ${showSettings ? '!text-[var(--accent)]' : ''}`}
            title="Engine & API settings"
          >
            <Settings size={14} />
          </button>
        </div>
      </div>

      {/* Recent threads popover */}
      {showThreads && (
        <div
          ref={threadsRef}
          className="absolute left-2 top-10 z-40 w-[300px] max-w-[calc(100%-16px)] rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl py-1 text-[12px]"
        >
          <div className={menuHead}>Recent threads</div>
          {threadStore.list.length === 0 ? (
            <div className="px-3 py-2 text-[var(--text-dim)]">No saved threads in this workspace yet.</div>
          ) : (
            <div className="max-h-64 overflow-y-auto">
              {threadStore.list.map((t) => (
                <div
                  key={t.id}
                  onClick={() => openThread(t)}
                  className={`group ${menuRow} ${t.id === threadId ? 'bg-[var(--bg-active)]' : ''}`}
                >
                  <span className="truncate flex-1" title={t.title}>
                    {t.title}
                  </span>
                  <span className="shrink-0 text-[11px] text-[var(--text-dim)] group-hover:hidden">
                    {timeAgo(t.updatedAt)}
                  </span>
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      removeThread(t.id);
                    }}
                    className="shrink-0 hidden group-hover:block text-[var(--text-dim)] hover:text-[var(--danger)] cursor-pointer"
                    title="Delete thread"
                  >
                    <X size={12} />
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Settings popover (overlay, never shifts the chat) */}
      {showSettings && (
        <div
          ref={settingsRef}
          className="absolute right-2 top-10 z-40 w-[320px] max-w-[calc(100%-16px)] p-3 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl space-y-2.5 text-xs"
        >
          <div className="flex items-center justify-between font-semibold text-[var(--text-primary)]">
            <span>Engine & API</span>
            <button
              onClick={() => setShowSettings(false)}
              className="text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
            >
              <X size={13} />
            </button>
          </div>
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="block text-[11px] text-[var(--text-muted)] mb-0.5">Provider</label>
              <select
                value={config.provider}
                onChange={(e) => setProvider(e.target.value as Provider)}
                className={inputCls}
              >
                <option value="openai">OpenAI / Compatible</option>
                <option value="anthropic">Anthropic Claude</option>
                <option value="gemini">Google Gemini</option>
                <option value="ollama">Ollama (Local)</option>
                <option value="claude-cli">Claude Code (installed CLI)</option>
                <option value="codex-cli">Codex (installed CLI)</option>
              </select>
            </div>
            <div>
              <label className="block text-[11px] text-[var(--text-muted)] mb-0.5">Model</label>
              <input
                value={config.model}
                onChange={(e) => setConfig((prev) => ({ ...prev, model: e.target.value }))}
                className={inputCls}
              />
            </div>
          </div>
          {cliToolId && (
            <div className="text-[11px] leading-snug text-[var(--text-muted)]">
              {cliInstalled === false
                ? `${PROVIDER_LABEL[config.provider]} CLI not found. Install it, then reopen this menu.`
                : `Runs your installed ${PROVIDER_LABEL[config.provider]} CLI with its own login — no API key. File edits are auto-accepted; use "default" to keep the CLI's configured model.`}
            </div>
          )}
          {config.provider === 'ollama' && localModels && !localModels.ollama.running && (
            <div className="text-[11px] leading-snug text-[var(--text-muted)]">
              Ollama isn&apos;t running at this address, so the model list is a placeholder.
            </div>
          )}
          {config.provider !== 'gemini' && !cliToolId && (
            <div>
              <label className="block text-[11px] text-[var(--text-muted)] mb-0.5">
                Base URL (optional — OpenRouter, DeepSeek, LM Studio)
              </label>
              <input
                value={config.baseUrl}
                onChange={(e) => setConfig((prev) => ({ ...prev, baseUrl: e.target.value }))}
                placeholder="https://api.openai.com/v1"
                className={inputCls}
              />
            </div>
          )}
          {config.provider !== 'ollama' && !cliToolId && (
            <div>
              <label className="block text-[11px] text-[var(--text-muted)] mb-0.5">
                API key (stored only in this browser)
              </label>
              <input
                type="password"
                value={config.apiKey}
                onChange={(e) => setConfig((prev) => ({ ...prev, apiKey: e.target.value }))}
                placeholder="sk-..."
                className={inputCls}
              />
            </div>
          )}
          <label className="flex items-center gap-2 text-[11px] text-[var(--text-muted)] cursor-pointer">
            <input
              type="checkbox"
              checked={config.autoCheckpoint}
              onChange={(e) => setConfig((prev) => ({ ...prev, autoCheckpoint: e.target.checked }))}
            />
            Auto-create Git checkpoint before agent edits
          </label>
        </div>
      )}

      {/* Zone 2: timeline */}
      <div className="flex-1 min-h-0 overflow-y-auto px-3 py-2.5 flex flex-col">
        {messages.length === 0 ? (
          // Zone 2A: ready state, anchored to the composer instead of floating mid-panel.
          <div className="mt-auto space-y-3">
            <button
              onClick={() => (engineStatus.ok ? undefined : setShowModelMenu(true))}
              data-pop="engine"
              className={`block w-full text-left px-3 py-2 rounded-md border border-[var(--border)] bg-[var(--bg-elevated)]/60 ${
                engineStatus.ok ? 'cursor-default' : 'cursor-pointer hover:bg-[var(--bg-hover)]'
              }`}
            >
              <div className="flex items-center gap-2 text-[12.5px]">
                <span
                  className="w-1.5 h-1.5 rounded-full shrink-0"
                  style={{ background: engineStatus.ok ? OK_DOT : WARN_DOT }}
                />
                <span className="font-medium text-[var(--text-primary)] truncate">{engineName}</span>
                <span className="flex-1" />
                <span
                  className="shrink-0 text-[11.5px]"
                  style={{ color: engineStatus.ok ? OK_DOT : WARN_DOT }}
                >
                  {engineStatus.text}
                  {!engineStatus.ok && ' · Choose engine →'}
                </span>
              </div>
              <div className="mt-0.5 pl-3.5 text-[11.5px] text-[var(--text-muted)] truncate">
                Workspace: {workspaceName}
                {branch && ` · ${branch}`}
                {changedFileCount > 0 && ` · ${changedFileCount} file${changedFileCount === 1 ? '' : 's'} changed`}
              </div>
            </button>

            <div>
              <div className="mb-1 text-[10.5px] font-semibold uppercase tracking-wider text-[var(--text-dim)]">
                Quick actions
              </div>
              <div className="rounded-md border border-[var(--border)] overflow-hidden">
                {quickActions.map((a, i) => (
                  <button
                    key={a.label}
                    onClick={a.run}
                    className={`group w-full flex items-center gap-2 h-[30px] px-2.5 text-left text-[12px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors ${
                      i > 0 ? 'border-t border-[var(--border)]' : ''
                    }`}
                  >
                    <span className="shrink-0 text-[var(--text-dim)] group-hover:text-[var(--text-muted)]">{a.icon}</span>
                    <span className="flex-1 truncate">{a.label}</span>
                    <span className="shrink-0 text-[var(--text-dim)] opacity-0 group-hover:opacity-100 transition-opacity">
                      {a.external ? <ArrowUpRight size={12} /> : <ArrowRight size={12} />}
                    </span>
                  </button>
                ))}
              </div>
            </div>
          </div>
        ) : (
          // Zone 2B: conversation
          <div className="space-y-3">
            {messages.map((m) => {
              if (m.role === 'user') {
                const ctx = m.ctxFiles ?? [];
                return (
                  <div
                    key={m.id}
                    className="rounded-md px-3 py-2 bg-[var(--bg-elevated)] border border-[var(--border)]"
                  >
                    {(ctx.length > 0 || m.ctxChanges) && (
                      <div className="flex flex-wrap gap-1 mb-1.5">
                        {ctx.map((f) => (
                          <ContextBadge
                            key={f}
                            icon={<FileCode size={10} className="shrink-0" />}
                            label={baseName(f)}
                            title={f}
                          />
                        ))}
                        {!!m.ctxChanges && (
                          <ContextBadge
                            icon={<GitCompare size={10} className="shrink-0" />}
                            label={`${m.ctxChanges} changed`}
                          />
                        )}
                      </div>
                    )}
                    <div className="text-[13px] leading-[1.5] text-[var(--text-primary)] whitespace-pre-wrap break-words">
                      {m.content}
                    </div>
                  </div>
                );
              }

              const tools = m.tools ?? [];
              const pending = tools.some((t) => t.result === undefined);
              const thoughtMs =
                m.startedAt !== undefined && m.firstOutputAt !== undefined ? m.firstOutputAt - m.startedAt : 0;
              const touched = !m.streaming ? editedPaths(tools) : [];
              return (
                <div key={m.id} className="space-y-2">
                  {thoughtMs >= 1000 && (
                    <div className="flex items-center h-6 text-[12px] text-[var(--text-dim)]">
                      Thought for {formatDur(thoughtMs)}
                    </div>
                  )}

                  {tools.length > 0 && (
                    <ToolGroup
                      tools={tools}
                      turnActive={!!m.streaming}
                      durationMs={m.startedAt !== undefined && m.endedAt !== undefined ? m.endedAt - m.startedAt : undefined}
                      expanded={expandedTools}
                      onToggle={(id) => setExpandedTools((prev) => ({ ...prev, [id]: !prev[id] }))}
                      onOpenDiff={onOpenDiff}
                      changeFor={changeFor}
                    />
                  )}

                  {m.content && <MarkdownMessage content={m.content} theme={theme} />}

                  {m.streaming && !pending && (
                    <div className="flex items-center h-6 text-[12px] text-[var(--text-dim)] animate-pulse">
                      Thinking…
                    </div>
                  )}

                  {touched.length > 0 && (
                    <TouchedFiles paths={touched} changeFor={changeFor} onOpenDiff={onOpenDiff} />
                  )}

                  {m.error && (
                    <div className="p-2 rounded bg-[var(--removed-bg)] text-[var(--removed-fg)] text-xs break-words">
                      {m.error}
                    </div>
                  )}

                  {m.checkpoint && (
                    <CheckpointDivider
                      checkpoint={m.checkpoint}
                      onUndo={() => void undoCheckpoint(m.checkpoint!.id)}
                      onRedo={() => void redoCheckpoint(m.checkpoint!.id)}
                      onForce={() => void undoCheckpoint(m.checkpoint!.id, true)}
                    />
                  )}
                </div>
              );
            })}
          </div>
        )}
        <div ref={endRef} />
      </div>

      {/* Zone 3: live change bar */}
      {liveEdits.length > 0 && (
        <div className="flex items-center gap-2 h-7 px-3 shrink-0 border-t border-[var(--border)] bg-[var(--bg-elevated)]/90 text-[11.5px] text-[var(--text-muted)]">
          <GitBranch size={12} className="shrink-0" />
          <span className="truncate">
            {liveEdits.length} file{liveEdits.length === 1 ? '' : 's'} edited{' '}
            <span className="font-mono tabular-nums">
              (<span className="text-[var(--added-fg)]">+{editTotals.adds}</span>{' '}
              <span className="text-[var(--removed-fg)]">-{editTotals.dels}</span>)
            </span>
          </span>
          <span className="flex-1" />
          <button
            onClick={() => onOpenDiff(liveEdits[liveEdits.length - 1].path)}
            className="shrink-0 px-1.5 h-5 rounded text-[var(--accent)] hover:bg-[var(--bg-hover)] cursor-pointer"
          >
            Review Diffs
          </button>
          {lastCheckpoint && !lastCheckpoint.redo_files?.length && (
            <button
              onClick={() => void undoCheckpoint(lastCheckpoint.id)}
              className="shrink-0 px-1.5 h-5 rounded hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
              title="Undo the latest agent turn (only the files it changed)"
            >
              Undo
            </button>
          )}
        </div>
      )}

      {/* Zone 4: composer */}
      <div className="px-3 pb-3 pt-2 shrink-0">
        <div className="relative rounded-lg border border-[var(--border)] bg-[var(--bg-input)] focus-within:border-[var(--accent)] transition-colors">
          {/* Row 1: context strip */}
          <div className="flex flex-wrap items-center gap-1 px-2.5 pt-2">
            {activeChip && (
              <span className="flex items-center gap-1 max-w-full h-5 pl-1.5 pr-1 rounded bg-[var(--bg-elevated)] text-[11px] text-[var(--text-primary)]">
                <FileCode size={11} className="shrink-0 text-[var(--text-muted)]" />
                <span className="truncate" title={activeChip}>
                  {baseName(activeChip)}
                </span>
                <button
                  onClick={() => setDismissedActive(activeChip)}
                  className="text-[var(--text-dim)] hover:text-[var(--text-primary)] cursor-pointer"
                  title="Remove from context"
                >
                  <X size={11} />
                </button>
              </span>
            )}
            {extraFiles.map((f) => (
              <span
                key={f}
                className="flex items-center gap-1 max-w-full h-5 pl-1.5 pr-1 rounded bg-[var(--bg-elevated)] text-[11px] text-[var(--text-primary)]"
              >
                <AtSign size={11} className="shrink-0 text-[var(--text-muted)]" />
                <span className="truncate" title={f}>
                  {baseName(f)}
                </span>
                <button
                  onClick={() => setExtraFiles((prev) => prev.filter((x) => x !== f))}
                  className="text-[var(--text-dim)] hover:text-[var(--text-primary)] cursor-pointer"
                  title="Remove from context"
                >
                  <X size={11} />
                </button>
              </span>
            ))}
            {changedFileCount > 0 && (
              <button
                onClick={() => setIncludeChanges((v) => !v)}
                className={`h-5 px-1.5 rounded border text-[11px] cursor-pointer transition-colors ${
                  includeChanges
                    ? 'border-[var(--accent)] bg-[var(--bg-active)] text-[var(--text-primary)]'
                    : 'border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
                }`}
                title="Tell the agent about uncommitted changes"
              >
                ± {changedFileCount} changed
              </button>
            )}
            <button
              data-pop="picker"
              onClick={() => {
                setFileQuery('');
                setAtRange(null);
                setShowFilePicker((v) => !v);
              }}
              className="flex items-center gap-1 h-5 px-1.5 rounded border border-[var(--border)] text-[11px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
              title="Add a file to the context (or type @)"
            >
              <Plus size={10} />
              Context
            </button>
          </div>

          {pickerOpen && (
            <div
              ref={pickerRef}
              className="absolute left-2 right-2 bottom-full mb-1 z-40 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl overflow-hidden"
            >
              {!atRange && (
                <input
                  autoFocus
                  value={fileQuery}
                  onChange={(e) => setFileQuery(e.target.value)}
                  placeholder="Search files…"
                  className="w-full px-3 py-2 text-[12px] bg-transparent border-b border-[var(--border)] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none"
                />
              )}
              <div className="max-h-52 overflow-y-auto py-1">
                {filteredFiles.length === 0 ? (
                  <div className="px-3 py-2 text-[12px] text-[var(--text-dim)]">No matching files</div>
                ) : (
                  filteredFiles.map((f, i) => (
                    <div
                      key={f}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => addContextFile(f)}
                      className={`px-3 py-1 text-[12px] font-mono truncate cursor-pointer hover:bg-[var(--bg-hover)] text-[var(--text-primary)] ${
                        atRange && i === pickIdx ? 'bg-[var(--bg-active)]' : ''
                      }`}
                      title={f}
                    >
                      {f}
                    </div>
                  ))
                )}
              </div>
            </div>
          )}

          {/* Row 2: textarea */}
          <textarea
            ref={textareaRef}
            value={input}
            onChange={(e) => {
              const v = e.target.value;
              setInput(v);
              const caret = e.target.selectionStart ?? v.length;
              const m = v.slice(0, caret).match(/(?:^|\s)@([^\s@]*)$/);
              setAtRange(m ? { start: caret - m[1].length - 1, end: caret, query: m[1] } : null);
              if (m) setShowFilePicker(false);
            }}
            onKeyDown={(e) => {
              if (atRange && filteredFiles.length > 0) {
                if (e.key === 'ArrowDown') {
                  e.preventDefault();
                  setPickIdx((i) => Math.min(i + 1, filteredFiles.length - 1));
                  return;
                }
                if (e.key === 'ArrowUp') {
                  e.preventDefault();
                  setPickIdx((i) => Math.max(i - 1, 0));
                  return;
                }
                if ((e.key === 'Enter' && !e.shiftKey) || e.key === 'Tab') {
                  e.preventDefault();
                  addContextFile(filteredFiles[Math.min(pickIdx, filteredFiles.length - 1)]);
                  return;
                }
              }
              if (e.key === 'Escape' && atRange) {
                setAtRange(null);
                return;
              }
              if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                sendPrompt();
              } else if (e.key === 'Escape' && running) {
                stopGeneration();
              }
            }}
            rows={1}
            style={{ minHeight: 44 }}
            placeholder="Ask anything, edit files, or explain diffs… (@ to add a file)"
            className="block w-full px-2.5 py-1.5 text-[13px] leading-[1.5] bg-transparent text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none resize-none"
          />

          {/* Row 3: control bar */}
          <div className="flex items-center justify-between gap-2 h-7 px-2 pb-1.5 box-content">
            <div className="relative min-w-0">
              <button
                data-pop="engine"
                onClick={() => setShowModelMenu((v) => !v)}
                className="flex items-center gap-1.5 max-w-full h-6 px-1.5 rounded-md text-[11.5px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                title={`${engineName} · ${config.model} — ${engineStatus.text}`}
              >
                <span
                  className="w-1.5 h-1.5 rounded-full shrink-0"
                  style={{ background: engineStatus.ok ? OK_DOT : WARN_DOT }}
                />
                <span className="truncate">{engineShort}</span>
                <ChevronDown size={11} className="shrink-0" />
              </button>
              {showModelMenu && (
                <div
                  ref={modelRef}
                  className="absolute left-0 bottom-full mb-1 z-40 w-[280px] max-w-[calc(100vw-32px)] max-h-[70vh] overflow-y-auto rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl py-1 text-[12px]"
                >
                  <div className={menuHead}>Installed CLI agents · no API key</div>
                  {CLI_PROVIDERS.map((p) => {
                    const installed = cliTools.find((t) => t.id === CLI_PROVIDER_TOOL[p])?.installed;
                    return (
                      <div key={p} onClick={() => chooseCli(p)} className={menuRow}>
                        <span
                          className="w-1.5 h-1.5 rounded-full shrink-0 border"
                          style={
                            installed === false
                              ? { borderColor: 'var(--text-dim)' }
                              : { background: OK_DOT, borderColor: OK_DOT }
                          }
                        />
                        <span className="flex-1 truncate">{PROVIDER_LABEL[p]} CLI</span>
                        <span className="text-[11px] text-[var(--text-dim)]">
                          {installed === false ? 'Not found' : installed ? 'Installed' : ''}
                        </span>
                        {config.provider === p && <Check size={12} className="text-[var(--accent)] shrink-0" />}
                      </div>
                    );
                  })}

                  <div className="my-1 border-t border-[var(--border)]" />
                  <div className={menuHead}>
                    Local models · Ollama {localModels ? (localModels.ollama.running ? '(running)' : '(not running)') : ''}
                  </div>
                  {ollamaModels.length === 0 ? (
                    <div className="px-3 py-1 text-[11.5px] text-[var(--text-dim)]">
                      {localModels?.ollama.running ? 'No models pulled yet.' : 'Start Ollama to list local models.'}
                    </div>
                  ) : (
                    ollamaModels.map((m) => (
                      <div key={m} onClick={() => chooseOllamaModel(m)} className={menuRow}>
                        <span className="w-1.5 h-1.5 rounded-full shrink-0" style={{ background: OK_DOT }} />
                        <span className="flex-1 truncate font-mono text-[11.5px]">{m}</span>
                        {config.provider === 'ollama' && config.model === m && (
                          <Check size={12} className="text-[var(--accent)] shrink-0" />
                        )}
                      </div>
                    ))
                  )}

                  <div className="my-1 border-t border-[var(--border)]" />
                  <div className={menuHead}>Direct API key</div>
                  {API_PROVIDERS.map((p) => {
                    const hasKey = !!(apiKeys[p] ?? '').trim();
                    return (
                      <div key={p} onClick={() => chooseApi(p)} className={menuRow}>
                        <span
                          className="w-1.5 h-1.5 rounded-full shrink-0 border"
                          style={
                            hasKey
                              ? { background: OK_DOT, borderColor: OK_DOT }
                              : { borderColor: 'var(--text-dim)' }
                          }
                        />
                        <span className="flex-1 truncate">
                          {PROVIDER_LABEL[p]} <span className="text-[var(--text-dim)]">· {PROVIDER_PRESETS[p].model.replace(/-\d{8}$/, '')}</span>
                        </span>
                        {config.provider === p && hasKey ? (
                          <Check size={12} className="text-[var(--accent)] shrink-0" />
                        ) : (
                          !hasKey && <span className="text-[11px] text-[var(--accent)] shrink-0">Set key →</span>
                        )}
                      </div>
                    );
                  })}

                  {config.provider !== 'ollama' && (
                    <>
                      <div className="my-1 border-t border-[var(--border)]" />
                      <div className={menuHead}>Model · {PROVIDER_LABEL[config.provider]}</div>
                      {modelOptions.map((m) => (
                        <div
                          key={m}
                          onClick={() => {
                            setConfig((prev) => ({ ...prev, model: m }));
                            setShowModelMenu(false);
                          }}
                          className={menuRow}
                        >
                          <span className="truncate flex-1 font-mono text-[11.5px]">{m}</span>
                          {config.model === m && <Check size={12} className="text-[var(--accent)] shrink-0" />}
                        </div>
                      ))}
                    </>
                  )}
                  <form
                    onSubmit={(e) => {
                      e.preventDefault();
                      const v = customModel.trim();
                      if (!v) return;
                      setConfig((prev) => ({ ...prev, model: v }));
                      setCustomModel('');
                      setShowModelMenu(false);
                    }}
                    className="px-2 py-1"
                  >
                    <input
                      value={customModel}
                      onChange={(e) => setCustomModel(e.target.value)}
                      placeholder="Custom model id…"
                      className="w-full px-2 py-1 text-[11.5px] font-mono rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none focus:border-[var(--accent)]"
                    />
                  </form>

                  <div className="my-1 border-t border-[var(--border)]" />
                  <div
                    onClick={() => {
                      setShowModelMenu(false);
                      setShowSettings(true);
                    }}
                    className={`${menuRow} !text-[var(--text-muted)]`}
                  >
                    <Settings size={12} />
                    <span className="flex-1">Configure paths & API keys…</span>
                  </div>
                </div>
              )}
            </div>

            <div className="flex items-center gap-1 shrink-0">
              <button
                onClick={onLaunchCliAgent}
                className="w-6 h-6 flex items-center justify-center rounded-md text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                title="Open an interactive CLI agent in the terminal dock"
              >
                <TerminalIcon size={13} />
              </button>
              <button
                onClick={() => setConfig((prev) => ({ ...prev, autoCheckpoint: !prev.autoCheckpoint }))}
                className={`flex items-center gap-0.5 h-6 px-1.5 rounded-md text-[11px] cursor-pointer transition-colors ${
                  config.autoCheckpoint
                    ? 'text-[var(--text-primary)] bg-[var(--bg-hover)]'
                    : 'text-[var(--text-dim)] hover:bg-[var(--bg-hover)]'
                }`}
                title={`Auto-checkpoint before agent edits: ${config.autoCheckpoint ? 'on' : 'off'}`}
              >
                <Zap size={11} />
                Auto
              </button>
              {running ? (
                <button
                  onClick={stopGeneration}
                  className="w-6 h-6 flex items-center justify-center rounded-md bg-[var(--danger)] text-white cursor-pointer"
                  title="Stop (Esc)"
                >
                  <Square size={11} />
                </button>
              ) : (
                <button
                  onClick={sendPrompt}
                  disabled={!input.trim()}
                  className="w-6 h-6 flex items-center justify-center rounded-md bg-[var(--accent)] text-white disabled:opacity-40 cursor-pointer hover:opacity-95 transition-opacity"
                  title="Send (Enter)"
                >
                  <ArrowUp size={13} />
                </button>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};
