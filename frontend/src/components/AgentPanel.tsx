import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import {
  ArrowRight,
  ArrowUp,
  ArrowUpRight,
  ArrowDown,
  AtSign,
  Bug,
  Check,
  ChevronDown,
  FileCode,
  FolderOpen,
  GitBranch,
  GitCompare,
  History,
  Lock,
  Plus,
  RotateCcw,
  Undo2,
  Settings,
  Square,
  Terminal as TerminalIcon,
  X,
  Zap,
} from 'lucide-react';
import { api, streamAgentChat } from '../api';
import {
  CLI_PROVIDERS,
  CLI_PROVIDER_TOOL,
  API_PROVIDERS,
  PERM_MODES,
  PROVIDER_LABEL,
  PROVIDER_PRESETS,
  useAgentSettings,
  type Provider,
} from '../agentSettings';
import type { LiteTheme } from '../themes';
import type {
  Checkpoint,
  GitFileStatus,
  MessagePart,
  PermissionMode,
  PermissionRequest,
  QuestionRequest,
  RollbackRes,
  SlashCommand,
  ToastAction,
  UiChatMessage,
} from '../types';
import { MarkdownMessage } from './MarkdownView';
import {
  ActivityGroup,
  DiffStat,
  LiveStatus,
  baseName,
  editedPaths,
  formatDur,
  normPath,
  type ChangeLookup,
} from './AgentActivity';
import { PermissionCard } from './agent/PermissionCard';
import { QuestionCard } from './agent/QuestionCard';
import { SlashCommandMenu } from './agent/SlashCommandMenu';
import { sessionKey, supportsSessions } from './agent/sessions';
import { useSlashMenu } from './agent/useSlashMenu';

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

const shortModel = (m: string) => (m === 'default' ? '' : m.replace(/-\d{8}$/, ''));

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
  /** The agent CLI's own conversation id per engine, so the thread resumes after a restart. */
  resumeIds?: Record<string, string>;
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

/** The resume ids belonging to one thread (keys are `thread:engine`). */
const threadResumeIds = (all: Record<string, string>, threadId: string): Record<string, string> =>
  Object.fromEntries(Object.entries(all).filter(([k]) => k.startsWith(`${threadId}:`)));

const threadTitle = (ms: UiChatMessage[]) => {
  const first = ms.find((m) => m.role === 'user');
  return first ? first.content.replace(/\s+/g, ' ').trim().slice(0, 60) : 'New thread';
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
  /** Open the Settings tab (engine, models, API keys, preferences). */
  onOpenSettings: () => void;
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
  onOpenSettings,
}) => {
  const [showModelMenu, setShowModelMenu] = useState(false);
  const [showThreads, setShowThreads] = useState(false);
  const [showFilePicker, setShowFilePicker] = useState(false);
  const [fileQuery, setFileQuery] = useState('');
  const [atRange, setAtRange] = useState<AtRange | null>(null);
  const [pickIdx, setPickIdx] = useState(0);
  const [customModel, setCustomModel] = useState('');
  // Live tool-approval prompts from a running Claude Code turn. Never persisted with the thread.
  const [permRequests, setPermRequests] = useState<PermissionRequest[]>([]);
  const [showPermMenu, setShowPermMenu] = useState(false);
  // "Always allow in this thread" rules; the CLI runs statelessly, so Vireo re-sends them each turn.
  const threadRulesRef = useRef<Record<string, string[]>>({});
  // Multiple-choice questions from the running agent turn. Never persisted with the thread.
  const [questions, setQuestions] = useState<QuestionRequest[]>([]);
  // The agent session's slash commands for the current thread + engine (fetched lazily).
  const [commands, setCommands] = useState<SlashCommand[]>([]);
  const [commandsLoading, setCommandsLoading] = useState(false);
  // Agent conversation ids per `thread:engine`, sent back so a restarted backend can resume them.
  const resumeIdsRef = useRef<Record<string, string>>({});

  const {
    config,
    setConfig,
    apiKeys,
    setProvider,
    cliTools,
    localModels,
    ollamaModels,
    modelOptions,
    refresh: refreshEngines,
    showQuickActions,
  } = useAgentSettings();

  const [messages, setMessages] = useState<UiChatMessage[]>([]);
  const [threadId, setThreadId] = useState(newThreadId);
  const [threadStore, setThreadStore] = useState(() => ({
    ws: workspacePath,
    list: loadThreads(workspacePath),
  }));
  const [input, setInput] = useState('');
  const [running, setRunning] = useState(false);
  const [expandedTools, setExpandedTools] = useState<Record<string, boolean>>({});
  // Activity groups the user opened/closed by hand, keyed `${messageId}:${partIndex}`.
  // Untouched groups follow the default: open while they are the live step, collapsed after.
  const [groupOpen, setGroupOpen] = useState<Record<string, boolean>>({});
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickRef = useRef(true);
  const [atBottom, setAtBottom] = useState(true);
  // Context chips: the active file is on by default (per-path dismiss); changes are opt-in.
  const [dismissedActive, setDismissedActive] = useState<string | null>(null);
  const [includeChanges, setIncludeChanges] = useState(false);
  const [extraFiles, setExtraFiles] = useState<string[]>([]);
  const abortRef = useRef<AbortController | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const sessionEngine = supportsSessions(config.provider);
  const loadCommands = React.useCallback(() => {
    if (!sessionEngine || commands.length > 0 || commandsLoading) return;
    setCommandsLoading(true);
    api
      .prepareSession({
        threadId,
        provider: config.provider,
        model: config.model,
        permissionMode: config.permissionMode ?? 'auto_edit',
        resumeId: resumeIdsRef.current[sessionKey(threadId, config.provider)],
        allowedRules: threadRulesRef.current[threadId],
      })
      .then((res) => {
        if (res.resume_id) resumeIdsRef.current[sessionKey(threadId, config.provider)] = res.resume_id;
        setCommands(res.commands);
      })
      .catch(() => {})
      .finally(() => setCommandsLoading(false));
  }, [sessionEngine, commands.length, commandsLoading, threadId, config.provider, config.model, config.permissionMode]);
  const slash = useSlashMenu(sessionEngine ? commands : [], loadCommands);

  // Commands belong to one agent session: forget them when the thread or engine changes.
  useEffect(() => {
    setCommands([]);
    slash.close();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [threadId, config.provider]);

  const modelRef = usePopoverClose(showModelMenu, () => setShowModelMenu(false), 'engine');
  const threadsRef = usePopoverClose(showThreads, () => setShowThreads(false), 'threads');
  const permMenuRef = usePopoverClose(showPermMenu, () => setShowPermMenu(false), 'perm');
  const pickerOpen = showFilePicker || atRange !== null;
  const pickerRef = usePopoverClose(
    pickerOpen,
    () => {
      setShowFilePicker(false);
      setAtRange(null);
    },
    'picker'
  );

  // Re-detect installed CLIs / models whenever the engine menu opens.
  useEffect(() => {
    if (showModelMenu) refreshEngines();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showModelMenu]);

  const cliToolId = CLI_PROVIDER_TOOL[config.provider];
  const cliInstalled = cliToolId ? cliTools.find((t) => t.id === cliToolId)?.installed : undefined;
  const configured = cliToolId
    ? cliInstalled !== false
    : config.provider === 'ollama' || !!config.apiKey.trim();
  const orderedCliProviders = useMemo(() => {
    return [...CLI_PROVIDERS].sort((a, b) => {
      const aInst = cliTools.find((t) => t.id === CLI_PROVIDER_TOOL[a])?.installed ? 1 : 0;
      const bInst = cliTools.find((t) => t.id === CLI_PROVIDER_TOOL[b])?.installed ? 1 : 0;
      return bInst - aInst;
    });
  }, [cliTools]);
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
    setGroupOpen({});
    stickRef.current = true;
    setAtBottom(true);
    setThreadId(newThreadId());
    setQuestions([]);
    resumeIdsRef.current = {};
    setThreadStore({ ws: workspacePath, list: loadThreads(workspacePath) });
  }, [workspacePath, threadStore.ws]);

  useEffect(() => {
    if (running || messages.length === 0) return;
    setThreadStore((prev) => ({
      ws: prev.ws,
      list: [
        {
          id: threadId,
          title: threadTitle(messages),
          updatedAt: Date.now(),
          messages: slimMessages(messages),
          resumeIds: threadResumeIds(resumeIdsRef.current, threadId),
        },
        ...prev.list.filter((t) => t.id !== threadId),
      ].slice(0, MAX_THREADS),
    }));
  }, [messages, running, threadId]);

  useEffect(() => {
    if (draftAppend?.text) {
      setInput((prev) => (prev ? `${prev}\n\n${draftAppend.text}` : draftAppend.text));
    }
  }, [draftAppend]);

  // Follow new output only while the user is parked at the bottom; never yank them back up/down
  // while they read earlier steps. (scrollIntoView would also scroll the IDE's outer panes.)
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && stickRef.current) el.scrollTop = el.scrollHeight;
  }, [messages]);

  const onTimelineScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    const near = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
    stickRef.current = near;
    if (near !== atBottom) setAtBottom(near);
  };

  const jumpToLatest = () => {
    const el = scrollRef.current;
    if (!el) return;
    stickRef.current = true;
    setAtBottom(true);
    el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
  };

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
    stickRef.current = true;
    setAtBottom(true);

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
              parts: [...(m.parts ?? []), { kind: 'text', text }],
              firstOutputAt: m.firstOutputAt ?? Date.now(),
            })),
          // Token-level streaming: fragments extend the current paragraph.
          onTextDelta: (delta) =>
            patchAssistant(assistantId, (m) => {
              const parts = [...(m.parts ?? [])];
              const last = parts[parts.length - 1];
              if (last?.kind === 'text') parts[parts.length - 1] = { kind: 'text', text: last.text + delta };
              else parts.push({ kind: 'text', text: delta.replace(/^\n+/, '') });
              return {
                ...m,
                content: m.content + (m.content ? delta : delta.replace(/^\n+/, '')),
                parts,
                firstOutputAt: m.firstOutputAt ?? Date.now(),
              };
            }),
          onSession: (sess) => {
            resumeIdsRef.current[sessionKey(threadId, config.provider)] = sess.resume_id;
          },
          onCommands: setCommands,
          onToolCall: (call) =>
            patchAssistant(assistantId, (m) => {
              const parts = [...(m.parts ?? [])];
              const last = parts[parts.length - 1];
              if (last?.kind === 'tools') parts[parts.length - 1] = { kind: 'tools', ids: [...last.ids, call.id] };
              else parts.push({ kind: 'tools', ids: [call.id] });
              return {
                ...m,
                tools: [...(m.tools || []), { id: call.id, name: call.name, args: call.args, startedAt: Date.now() }],
                parts,
                firstOutputAt: m.firstOutputAt ?? Date.now(),
              };
            }),
          onToolResult: (res) => {
            patchAssistant(assistantId, (m) => ({
              ...m,
              tools: (m.tools || []).map((t) =>
                t.id === res.id ? { ...t, result: res.result, endedAt: Date.now() } : t
              ),
            }));
            if (['write_file', 'edit_file', 'symbol_edit', 'ast'].includes(res.name)) {
              onRefreshGit();
            }
          },
          onCheckpoint: (cp) => {
            patchAssistant(assistantId, (m) => ({ ...m, checkpoint: cp }));
            onRefreshGit();
          },
          onPermissionRequest: (req) =>
            setPermRequests((prev) =>
              prev.some((r) => r.request_id === req.request_id) ? prev : [...prev, req]
            ),
          onPermissionCancel: (id) => setPermRequests((prev) => prev.filter((r) => r.request_id !== id)),
          onUserQuestion: (req) =>
            setQuestions((prev) => (prev.some((q) => q.request_id === req.request_id) ? prev : [...prev, req])),
          onError: (msg) => patchAssistant(assistantId, (m) => ({ ...m, error: msg })),
          onDone: () => {
            patchAssistant(assistantId, (m) => ({ ...m, streaming: false, endedAt: Date.now() }));
            setRunning(false);
            setPermRequests([]);
            setQuestions([]);
            onRefreshGit();
          },
        },
        ctrl.signal,
        threadRulesRef.current[threadId],
        supportsSessions(config.provider)
          ? { threadId, resumeId: resumeIdsRef.current[sessionKey(threadId, config.provider)] }
          : undefined
      );
    } catch (e: unknown) {
      if ((e as Error).name !== 'AbortError') {
        patchAssistant(assistantId, (m) => ({ ...m, error: String(e), streaming: false, endedAt: Date.now() }));
      }
      setRunning(false);
      setPermRequests([]);
      setQuestions([]);
    }
  };

  const answerPermission = (p: PermissionRequest, allow: boolean, remember = false) => {
    setPermRequests((prev) => prev.filter((r) => r.request_id !== p.request_id));
    if (allow && remember) {
      const rules = threadRulesRef.current[threadId] ?? [];
      if (!rules.includes(p.rule)) threadRulesRef.current[threadId] = [...rules, p.rule];
    }
    api.answerPermission(p.turn_id, p.request_id, allow, remember).catch(() => {});
  };

  const answerQuestion = (q: QuestionRequest, answers: Record<string, string[]>) => {
    setQuestions((prev) => prev.filter((r) => r.request_id !== q.request_id));
    api.answerQuestion(q.turn_id, q.request_id, answers).catch(() => {});
  };

  const dismissQuestion = (q: QuestionRequest) => {
    setQuestions((prev) => prev.filter((r) => r.request_id !== q.request_id));
    api.answerQuestion(q.turn_id, q.request_id, {}, true).catch(() => {});
  };

  const stopGeneration = () => {
    abortRef.current?.abort();
    setRunning(false);
    setPermRequests([]);
    setQuestions([]);
    setMessages((prev) =>
      prev.map((m) => (m.streaming ? { ...m, streaming: false, endedAt: Date.now() } : m))
    );
  };

  const newThread = () => {
    abortRef.current?.abort();
    setRunning(false);
    setPermRequests([]);
    setQuestions([]);
    setMessages([]);
    setExpandedTools({});
    setGroupOpen({});
    stickRef.current = true;
    setAtBottom(true);
    setThreadId(newThreadId());
    setShowThreads(false);
  };

  const openThread = (t: AgentThread) => {
    abortRef.current?.abort();
    setRunning(false);
    setPermRequests([]);
    setQuestions([]);
    Object.assign(resumeIdsRef.current, t.resumeIds ?? {});
    setMessages(t.messages);
    setExpandedTools({});
    setGroupOpen({});
    stickRef.current = true;
    setAtBottom(true);
    setThreadId(t.id);
    setShowThreads(false);
  };

  const removeThread = (id: string) => {
    setThreadStore((prev) => ({ ws: prev.ws, list: prev.list.filter((t) => t.id !== id) }));
    api.closeSession(id).catch(() => {});
    if (id === threadId) {
      abortRef.current?.abort();
      setRunning(false);
      setMessages([]);
      setThreadId(newThreadId());
    }
  };

  const pickSlash = (command: SlashCommand) => {
    setInput((prev) => slash.complete(prev, command));
    slash.close();
    requestAnimationFrame(() => textareaRef.current?.focus());
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
    if (!(apiKeys[p] ?? '').trim()) onOpenSettings();
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
            onClick={onOpenSettings}
            className={iconBtn}
            title="Settings"
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

      {/* Zone 2: timeline */}
      <div className="relative flex-1 min-h-0 flex flex-col">
        <div
          ref={scrollRef}
          onScroll={onTimelineScroll}
          className="flex-1 min-h-0 overflow-y-auto px-3 py-2.5 flex flex-col"
        >
        {messages.length === 0 ? (
          // Zone 2A: ready state, anchored to the composer instead of floating mid-panel.
          <div className="mt-auto space-y-3">
            {showQuickActions && (
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
            )}
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
              const pending = !!m.streaming && tools.some((t) => t.result === undefined);
              const touched = !m.streaming ? editedPaths(tools) : [];
              const byId = new Map(tools.map((t) => [t.id, t]));
              // Older saved threads have no `parts`: show their steps first, then the reply.
              const parts: MessagePart[] =
                m.parts ??
                [
                  ...(tools.length ? [{ kind: 'tools' as const, ids: tools.map((t) => t.id) }] : []),
                  ...(m.content ? [{ kind: 'text' as const, text: m.content }] : []),
                ];
              const worked =
                !m.streaming && tools.length > 0 && m.startedAt !== undefined && m.endedAt !== undefined
                  ? m.endedAt - m.startedAt
                  : undefined;
              return (
                <div key={m.id} className="space-y-2">
                  {parts.map((p, i) => {
                    if (p.kind === 'text') {
                      return <MarkdownMessage key={`t${i}`} content={p.text} theme={theme} />;
                    }
                    const key = `${m.id}:${i}`;
                    const groupTools = p.ids.map((id) => byId.get(id)).filter((t): t is NonNullable<typeof t> => !!t);
                    return (
                      <ActivityGroup
                        key={key}
                        tools={groupTools}
                        turnActive={!!m.streaming}
                        open={groupOpen[key] ?? (!!m.streaming && i === parts.length - 1)}
                        onOpenChange={(open) => setGroupOpen((prev) => ({ ...prev, [key]: open }))}
                        expanded={expandedTools}
                        onToggleTool={(id) => setExpandedTools((prev) => ({ ...prev, [id]: !prev[id] }))}
                        onOpenDiff={onOpenDiff}
                        changeFor={changeFor}
                      />
                    );
                  })}

                  {m.streaming && !pending && (
                    <LiveStatus label={parts.length ? 'Thinking' : 'Starting'} since={m.startedAt} />
                  )}

                  {worked !== undefined && worked >= 1000 && (
                    <div className="pl-[3px] text-[11px] text-[var(--text-dim)]">
                      Worked for {formatDur(worked)} · {tools.length} step{tools.length === 1 ? '' : 's'}
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
            {permRequests.map((p) => (
              <PermissionCard key={p.request_id} request={p} onAnswer={answerPermission} />
            ))}
            {questions.map((q) => (
              <QuestionCard key={q.request_id} request={q} onSubmit={answerQuestion} onDismiss={dismissQuestion} />
            ))}
          </div>
        )}
        </div>
        {!atBottom && messages.length > 0 && (
          <button
            onClick={jumpToLatest}
            className="absolute left-1/2 -translate-x-1/2 bottom-2 z-10 flex items-center gap-1 h-6 pl-2 pr-2.5 rounded-full border border-[var(--border)] bg-[var(--bg-elevated)] shadow-lg text-[11px] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
          >
            <ArrowDown size={12} />
            {running ? 'Live activity below' : 'Jump to latest'}
          </button>
        )}
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

          {slash.open && sessionEngine && (
            <SlashCommandMenu
              items={slash.items}
              selected={slash.index}
              loading={commandsLoading}
              onPick={pickSlash}
            />
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
              if (sessionEngine) slash.update(v, caret);
            }}
            onKeyDown={(e) => {
              if (slash.open && sessionEngine) {
                if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                  e.preventDefault();
                  slash.move(e.key === 'ArrowDown' ? 1 : -1);
                  return;
                }
                if (((e.key === 'Enter' && !e.shiftKey) || e.key === 'Tab') && slash.items.length > 0) {
                  e.preventDefault();
                  pickSlash(slash.items[slash.index]);
                  return;
                }
                if (e.key === 'Escape') {
                  slash.close();
                  return;
                }
              }
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
            placeholder={
              sessionEngine
                ? 'Ask anything, edit files, or explain diffs… (@ for files, / for commands)'
                : 'Ask anything, edit files, or explain diffs… (@ to add a file)'
            }
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
                  {orderedCliProviders.map((p) => {
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
                          {PROVIDER_LABEL[p]} {PROVIDER_PRESETS[p].model && <span className="text-[var(--text-dim)]">· {PROVIDER_PRESETS[p].model}</span>}
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
                      onOpenSettings();
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
              {CLI_PROVIDERS.includes(config.provider) && (
                <div className="relative">
                  <button
                    data-pop="perm"
                    onClick={() => setShowPermMenu((v) => !v)}
                    className="flex items-center gap-1 h-6 px-1.5 rounded-md text-[11px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                    title="What the agent may do without asking"
                  >
                    {(() => {
                      const Icon = PERM_MODES[config.permissionMode ?? 'auto_edit'].icon;
                      return <Icon size={11} />;
                    })()}
                    {PERM_MODES[config.permissionMode ?? 'auto_edit'].label}
                  </button>
                  {showPermMenu && (
                    <div
                      ref={permMenuRef}
                      className="absolute right-0 bottom-full mb-1 z-40 w-[260px] max-w-[calc(100vw-32px)] rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl py-1 text-[12px]"
                    >
                      {(Object.keys(PERM_MODES) as PermissionMode[]).map((mode) => {
                        const m = PERM_MODES[mode];
                        const Icon = m.icon;
                        const note =
                          config.provider === 'claude-cli'
                            ? m.claudeNote
                            : config.provider === 'codex-cli'
                              ? m.codexNote
                              : m.cliNote;
                        return (
                          <div
                            key={mode}
                            onClick={() => {
                              setConfig((prev) => ({ ...prev, permissionMode: mode }));
                              setShowPermMenu(false);
                            }}
                            className="flex items-start gap-2 px-3 py-1.5 cursor-pointer hover:bg-[var(--bg-hover)] text-[var(--text-primary)]"
                          >
                            <Icon size={12} className="mt-0.5 shrink-0" />
                            <div className="flex-1 min-w-0">
                              <div>{m.label}</div>
                              <div className="text-[11px] text-[var(--text-dim)]">{note}</div>
                            </div>
                            {(config.permissionMode ?? 'auto_edit') === mode && <Check size={12} className="mt-0.5" />}
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>
              )}
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
