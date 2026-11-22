import React, { useEffect, useLayoutEffect, useRef, useState } from 'react';
import {
  AlertCircle,
  AtSign,
  Check,
  ChevronDown,
  ChevronRight,
  FileCode,
  FolderOpen,
  GitCompare,
  Loader2,
  Plus,
  RotateCcw,
  Search,
  Send,
  Settings,
  Square,
  Terminal as TerminalIcon,
  Wrench,
  X,
} from 'lucide-react';
import { api, streamAgentChat } from '../api';
import type { LiteTheme } from '../themes';
import type { AgentConfig, CliToolStatus, LocalModelsInfo, ToolCallTrace, UiChatMessage } from '../types';
import { MarkdownMessage } from './MarkdownView';
import { VireoLogo } from './VireoLogo';

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

/** Close a popover on outside click / Escape. */
function usePopoverClose(open: boolean, onClose: () => void) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
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
  }, [open, onClose]);
  return ref;
}

const baseName = (p: string) => p.split(/[\\/]/).pop() || p;

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
        meta: s || e ? `L${s ?? 1}–${e ?? ''}` : undefined,
        path,
        edits: false,
      };
    }
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

const ToolRow: React.FC<{
  tool: ToolCallTrace;
  turnActive: boolean;
  open: boolean;
  onToggle: () => void;
  onOpenDiff: (path: string) => void;
}> = ({ tool, turnActive, open, onToggle, onOpenDiff }) => {
  const v = describeTool(tool);
  const done = tool.result !== undefined;
  const failed = done && /^error/i.test(tool.result!.trim());
  const detail =
    tool.name === 'run_command'
      ? `$ ${v.subject}${tool.result ? `\n\n${tool.result}` : ''}`
      : tool.result || '';
  return (
    <div>
      <div
        onClick={onToggle}
        className="group flex items-center gap-1.5 h-[26px] pr-1 rounded cursor-pointer hover:bg-[var(--bg-hover)] text-[12px] text-[var(--text-muted)]"
      >
        <v.Icon size={13} className="shrink-0 text-[var(--text-dim)]" />
        <span className="shrink-0">{v.verb}</span>
        <span
          className={`font-mono text-[11.5px] truncate min-w-0 ${
            v.path ? 'text-[var(--text-primary)]' : 'text-[var(--text-primary)]/90'
          }`}
          title={v.subject}
        >
          {v.path ? baseName(v.subject) : v.subject}
        </span>
        {v.meta && <span className="shrink-0 text-[11px] text-[var(--text-dim)]">{v.meta}</span>}
        <span className="flex-1" />
        {v.edits && v.path && (
          <button
            onClick={(e) => {
              e.stopPropagation();
              onOpenDiff(v.path!);
            }}
            className="shrink-0 px-1.5 text-[11px] rounded text-[var(--accent)] hover:underline cursor-pointer"
          >
            View diff ↗
          </button>
        )}
        {!done && turnActive ? (
          <Loader2 size={12} className="shrink-0 animate-spin text-[var(--accent)]" />
        ) : failed ? (
          <AlertCircle size={12} className="shrink-0 text-[var(--danger)]" />
        ) : done ? (
          <Check size={12} className="shrink-0 text-[var(--added-fg)]" />
        ) : null}
        {detail && (
          <span className="shrink-0 text-[var(--text-dim)] opacity-0 group-hover:opacity-100">
            {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
          </span>
        )}
      </div>
      {open && detail && (
        <pre className="my-1 ml-5 p-2 rounded bg-[var(--bg-panel)] border border-[var(--border)]/70 text-[11px] font-mono text-[var(--text-muted)] max-h-48 overflow-auto whitespace-pre-wrap break-words">
          {detail.length > 4000 ? `${detail.slice(0, 4000)}\n… (truncated)` : detail}
        </pre>
      )}
    </div>
  );
};

const CheckpointDivider: React.FC<{
  label: string;
  onUndo: () => void;
}> = ({ label, onUndo }) => (
  <div className="group flex items-center gap-2 my-1 text-[11px] text-[var(--text-dim)]">
    <div className="flex-1 h-px bg-[var(--border)]" />
    <span className="flex items-center gap-1 shrink-0">
      <RotateCcw size={10} />
      Checkpoint · {label}
    </span>
    <button
      onClick={onUndo}
      className="shrink-0 px-1.5 py-0.5 rounded text-[var(--accent)] hover:bg-[var(--bg-hover)] cursor-pointer"
    >
      Undo turn
    </button>
    <div className="w-4 h-px bg-[var(--border)]" />
  </div>
);

function formatCheckpointTime(raw: string): string {
  const d = new Date(raw);
  return Number.isNaN(d.getTime())
    ? raw
    : d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
}

interface AgentPanelProps {
  theme: LiteTheme;
  workspacePath: string;
  draftAppend: { id: number; text: string } | null;
  /** Active editor/diff file, offered as a context chip. */
  activeFilePath: string | null;
  changedFileCount: number;
  allFiles: string[];
  onLaunchCliAgent: () => void;
  onOpenDiff: (path: string) => void;
  onRefreshGit: () => Promise<void>;
  onToast: (msg: string, isError?: boolean) => void;
}

export const AgentPanel: React.FC<AgentPanelProps> = ({
  theme,
  draftAppend,
  activeFilePath,
  changedFileCount,
  allFiles,
  onLaunchCliAgent,
  onOpenDiff,
  onRefreshGit,
  onToast,
}) => {
  const [showSettings, setShowSettings] = useState(false);
  const [showModelMenu, setShowModelMenu] = useState(false);
  const [showFilePicker, setShowFilePicker] = useState(false);
  const [fileQuery, setFileQuery] = useState('');
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

  const [messages, setMessages] = useState<UiChatMessage[]>([]);
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

  const settingsRef = usePopoverClose(showSettings, () => setShowSettings(false));
  const modelRef = usePopoverClose(showModelMenu, () => setShowModelMenu(false));
  const pickerRef = usePopoverClose(showFilePicker, () => setShowFilePicker(false));

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
  const modelOptions: string[] =
    config.provider === 'ollama' && localModels?.ollama.running && localModels.ollama.models.length
      ? localModels.ollama.models
      : config.provider === 'codex-cli' && localModels?.codex.length
        ? ['default', ...localModels.codex]
        : MODEL_PRESETS[config.provider];
  const activeChip =
    activeFilePath && activeFilePath !== dismissedActive && !extraFiles.includes(activeFilePath)
      ? activeFilePath
      : null;

  useEffect(() => {
    try {
      localStorage.setItem('vireo-agent-config', JSON.stringify(config));
    } catch {
      // ignore
    }
  }, [config]);

  useEffect(() => {
    if (draftAppend?.text) {
      setInput((prev) => (prev ? `${prev}\n\n${draftAppend.text}` : draftAppend.text));
    }
  }, [draftAppend]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  // Auto-growing composer: ~2 rows up to 180px.
  useLayoutEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 180)}px`;
  }, [input]);

  const setProvider = (p: AgentConfig['provider']) =>
    setConfig((prev) => ({
      ...prev,
      provider: p,
      model:
        p === 'ollama' && localModels?.ollama.running && localModels.ollama.models.length
          ? localModels.ollama.models[0]
          : PROVIDER_PRESETS[p].model,
      baseUrl: PROVIDER_PRESETS[p].baseUrl,
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

  const sendPrompt = async () => {
    const trimmed = input.trim();
    if (!trimmed || running) return;

    const userMsg: UiChatMessage = {
      id: `u-${Date.now()}`,
      role: 'user',
      content: trimmed,
    };
    const assistantId = `a-${Date.now() + 1}`;
    const assistantMsg: UiChatMessage = {
      id: assistantId,
      role: 'assistant',
      content: '',
      tools: [],
      streaming: true,
    };

    const historyPayload = [
      ...messages.map((m) => ({ role: m.role, content: m.content })),
      { role: 'user', content: buildContextPrefix() + trimmed },
    ];

    setMessages((prev) => [...prev, userMsg, assistantMsg]);
    setInput('');
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
          onText: (text) => {
            setMessages((prev) =>
              prev.map((m) =>
                m.id === assistantId
                  ? { ...m, content: m.content ? `${m.content}\n\n${text}` : text }
                  : m
              )
            );
          },
          onToolCall: (call) => {
            setMessages((prev) =>
              prev.map((m) =>
                m.id === assistantId
                  ? {
                      ...m,
                      tools: [...(m.tools || []), { id: call.id, name: call.name, args: call.args }],
                    }
                  : m
              )
            );
          },
          onToolResult: (res) => {
            setMessages((prev) =>
              prev.map((m) =>
                m.id === assistantId
                  ? {
                      ...m,
                      tools: (m.tools || []).map((t) =>
                        t.id === res.id ? { ...t, result: res.result } : t
                      ),
                    }
                  : m
              )
            );
            if (res.name === 'write_file' || res.name === 'edit_file') {
              onRefreshGit();
            }
          },
          onCheckpoint: (cp) => {
            setMessages((prev) =>
              prev.map((m) => (m.id === assistantId ? { ...m, checkpoint: cp } : m))
            );
            onRefreshGit();
          },
          onError: (msg) => {
            setMessages((prev) =>
              prev.map((m) => (m.id === assistantId ? { ...m, error: msg } : m))
            );
          },
          onDone: () => {
            setMessages((prev) =>
              prev.map((m) => (m.id === assistantId ? { ...m, streaming: false } : m))
            );
            setRunning(false);
            onRefreshGit();
          },
        },
        ctrl.signal
      );
    } catch (e: unknown) {
      if ((e as Error).name !== 'AbortError') {
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId ? { ...m, error: String(e), streaming: false } : m
          )
        );
      }
      setRunning(false);
    }
  };

  const stopGeneration = () => {
    abortRef.current?.abort();
    setRunning(false);
    setMessages((prev) => prev.map((m) => (m.streaming ? { ...m, streaming: false } : m)));
  };

  const newThread = () => {
    abortRef.current?.abort();
    setRunning(false);
    setMessages([]);
    setExpandedTools({});
  };

  const prefill = (text: string) => {
    setInput(text);
    requestAnimationFrame(() => textareaRef.current?.focus());
  };

  const quickActions: { label: string; run: () => void }[] = [];
  if (changedFileCount > 0) {
    quickActions.push({
      label: 'Review uncommitted changes',
      run: () => {
        setIncludeChanges(true);
        prefill('Review my uncommitted changes. Run git diff, then point out bugs, risky edits and missing edge cases.');
      },
    });
  }
  if (activeFilePath) {
    quickActions.push({
      label: `Explain ${baseName(activeFilePath)}`,
      run: () =>
        prefill(`Explain what ${activeFilePath} does and how it fits into the rest of the project.`),
    });
    if (changedFileCount > 0) {
      quickActions.push({
        label: 'Find bugs in current diff',
        run: () =>
          prefill(`Look for bugs and edge cases in the uncommitted changes to ${activeFilePath}.`),
      });
    }
  }
  quickActions.push({ label: 'Launch Claude / Codex in terminal ↗', run: onLaunchCliAgent });

  const filteredFiles = (() => {
    const q = fileQuery.trim().toLowerCase();
    const pool = allFiles.filter((f) => !extraFiles.includes(f));
    return (q ? pool.filter((f) => f.toLowerCase().includes(q)) : pool).slice(0, 40);
  })();

  const undoCheckpoint = async (id: string) => {
    const res = await api.gitRollback(id);
    if (res.ok) {
      onToast('Rolled back agent changes!');
      onRefreshGit();
    } else {
      onToast(res.stderr || 'Rollback failed', true);
    }
  };

  const inputCls =
    'w-full px-2 py-1 rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none focus:border-[var(--accent)]';
  const iconBtn =
    'p-1.5 rounded-md text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors';

  return (
    <div className="relative flex flex-col h-full min-w-0 overflow-hidden bg-[var(--bg-sidebar)] border-l border-[var(--border)]">
      {/* Header */}
      <div className="flex items-center justify-between gap-1 px-3 h-10 border-b border-[var(--border)] shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          <span className="text-[12px] font-semibold tracking-wide uppercase text-[var(--text-muted)]">
            Vireo Agent
          </span>
          <span
            className="w-1.5 h-1.5 rounded-full shrink-0"
            style={{ background: configured ? '#10b981' : 'var(--text-dim)' }}
            title={
              configured
                ? `${PROVIDER_LABEL[config.provider]} ready`
                : cliToolId
                  ? `${PROVIDER_LABEL[config.provider]} CLI not found`
                  : 'No API key set for the selected provider'
            }
          />
        </div>
        <div className="flex items-center gap-0.5">
          <button onClick={newThread} className={iconBtn} title="New thread">
            <Plus size={15} />
          </button>
          <button onClick={onLaunchCliAgent} className={iconBtn} title="Launch CLI agent in the terminal dock">
            <TerminalIcon size={14} />
          </button>
          <button
            onClick={() => setShowSettings((v) => !v)}
            className={`${iconBtn} ${showSettings ? '!text-[var(--accent)]' : ''}`}
            title="Model & API settings"
          >
            <Settings size={15} />
          </button>
        </div>
      </div>

      {/* Settings popover (overlay, never shifts the chat) */}
      {showSettings && (
        <div
          ref={settingsRef}
          className="absolute right-2 top-11 z-40 w-[320px] max-w-[calc(100%-16px)] p-3 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl space-y-2.5 text-xs"
        >
          <div className="flex items-center justify-between font-semibold text-[var(--text-primary)]">
            <span>Model & API</span>
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
                onChange={(e) => setProvider(e.target.value as AgentConfig['provider'])}
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
              Ollama isn&apos;t running at this address, so the model list below is a placeholder.
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

      {/* Chat timeline */}
      <div className="flex-1 min-h-0 overflow-y-auto px-3.5 py-3 flex flex-col">
        {messages.length === 0 ? (
          <div className="flex-1 flex flex-col items-center justify-center text-center gap-3 pb-6">
            <VireoLogo size={30} className="opacity-80" />
            <div className="text-[14px] font-medium text-[var(--text-primary)]">
              What are we building or reviewing?
            </div>
            {!configured && (
              <button
                onClick={() => setShowSettings(true)}
                className="flex items-center gap-2 px-3 py-1.5 rounded-full border border-[var(--border)] bg-[var(--bg-elevated)] text-[12px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
              >
                Connect an API key or switch to local Ollama
                <span className="text-[var(--accent)] font-medium">Configure →</span>
              </button>
            )}
            <div className="flex flex-wrap justify-center gap-1.5 max-w-[340px]">
              {quickActions.map((a) => (
                <button
                  key={a.label}
                  onClick={a.run}
                  className="px-2.5 py-1 rounded-full border border-[var(--border)] text-[12px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                >
                  {a.label}
                </button>
              ))}
            </div>
          </div>
        ) : (
          <div className="space-y-4">
            {messages.map((m) =>
              m.role === 'user' ? (
                <div
                  key={m.id}
                  className="rounded-lg px-3 py-2 text-[13px] leading-[1.55] text-[var(--text-primary)] bg-[var(--bg-elevated)]/80 border border-[var(--border)]/70 whitespace-pre-wrap break-words"
                >
                  {m.content}
                </div>
              ) : (
                <div key={m.id} className="space-y-2">
                  {m.tools && m.tools.length > 0 && (
                    <div className="border-l border-[var(--border)] pl-2 ml-0.5">
                      {m.tools.map((t) => (
                        <ToolRow
                          key={t.id}
                          tool={t}
                          turnActive={!!m.streaming}
                          open={!!expandedTools[t.id]}
                          onToggle={() =>
                            setExpandedTools((prev) => ({ ...prev, [t.id]: !prev[t.id] }))
                          }
                          onOpenDiff={onOpenDiff}
                        />
                      ))}
                    </div>
                  )}

                  {m.content && <MarkdownMessage content={m.content} theme={theme} />}

                  {m.streaming && (
                    <div className="text-[12px] text-[var(--text-dim)] animate-pulse">
                      {m.tools && m.tools.some((t) => t.result === undefined)
                        ? 'Working…'
                        : 'Thinking…'}
                    </div>
                  )}

                  {m.error && (
                    <div className="p-2 rounded bg-[var(--removed-bg)] text-[var(--removed-fg)] text-xs break-words">
                      {m.error}
                    </div>
                  )}

                  {m.checkpoint && (
                    <CheckpointDivider
                      label={formatCheckpointTime(m.checkpoint.created_at)}
                      onUndo={() => undoCheckpoint(m.checkpoint!.id)}
                    />
                  )}
                </div>
              )
            )}
          </div>
        )}
        <div ref={endRef} />
      </div>

      {/* Composer */}
      <div className="px-3 pb-3 pt-1 shrink-0">
        <div className="relative rounded-lg border border-[var(--border)] bg-[var(--bg-input)] focus-within:border-[var(--accent)] transition-colors">
          {/* Context chips */}
          <div className="flex flex-wrap items-center gap-1.5 px-2.5 pt-2">
            {activeChip && (
              <span className="flex items-center gap-1 max-w-full pl-1.5 pr-1 py-0.5 rounded bg-[var(--bg-elevated)] border border-[var(--border)] text-[11.5px] text-[var(--text-primary)]">
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
                className="flex items-center gap-1 max-w-full pl-1.5 pr-1 py-0.5 rounded bg-[var(--bg-elevated)] border border-[var(--border)] text-[11.5px] text-[var(--text-primary)]"
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
                className={`px-1.5 py-0.5 rounded border text-[11.5px] cursor-pointer transition-colors ${
                  includeChanges
                    ? 'border-[var(--accent)] bg-[var(--bg-active)] text-[var(--text-primary)]'
                    : 'border-dashed border-[var(--border)] text-[var(--text-dim)] hover:text-[var(--text-primary)]'
                }`}
                title="Tell the agent about uncommitted changes"
              >
                ± {changedFileCount} changed
              </button>
            )}
            <button
              onClick={() => {
                setFileQuery('');
                setShowFilePicker((v) => !v);
              }}
              className="flex items-center gap-1 px-1.5 py-0.5 rounded border border-dashed border-[var(--border)] text-[11.5px] text-[var(--text-dim)] hover:text-[var(--text-primary)] cursor-pointer transition-colors"
              title="Add a file to the context"
            >
              <Plus size={10} />
              Context
            </button>
          </div>

          {showFilePicker && (
            <div
              ref={pickerRef}
              className="absolute left-2 right-2 bottom-full mb-1 z-40 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl overflow-hidden"
            >
              <input
                autoFocus
                value={fileQuery}
                onChange={(e) => setFileQuery(e.target.value)}
                placeholder="Search files…"
                className="w-full px-3 py-2 text-[12px] bg-transparent border-b border-[var(--border)] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none"
              />
              <div className="max-h-52 overflow-y-auto py-1">
                {filteredFiles.length === 0 ? (
                  <div className="px-3 py-2 text-[12px] text-[var(--text-dim)]">No matching files</div>
                ) : (
                  filteredFiles.map((f) => (
                    <div
                      key={f}
                      onClick={() => {
                        setExtraFiles((prev) => [...prev, f]);
                        setShowFilePicker(false);
                        textareaRef.current?.focus();
                      }}
                      className="px-3 py-1 text-[12px] font-mono truncate cursor-pointer hover:bg-[var(--bg-hover)] text-[var(--text-primary)]"
                      title={f}
                    >
                      {f}
                    </div>
                  ))
                )}
              </div>
            </div>
          )}

          <textarea
            ref={textareaRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                sendPrompt();
              } else if (e.key === 'Escape' && running) {
                stopGeneration();
              }
            }}
            rows={2}
            placeholder="Ask anything, edit files, or explain diffs…"
            className="block w-full px-3 py-2 text-[13px] leading-relaxed bg-transparent text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none resize-none"
          />

          <div className="flex items-center justify-between gap-2 px-2 pb-2">
            <div className="relative min-w-0">
              <button
                onClick={() => setShowModelMenu((v) => !v)}
                className="flex items-center gap-1 max-w-full px-2 py-1 rounded-md text-[11.5px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                title={`${PROVIDER_LABEL[config.provider]} · ${config.model}`}
              >
                <span className="truncate">{config.model}</span>
                <ChevronDown size={11} className="shrink-0" />
              </button>
              {showModelMenu && (
                <div
                  ref={modelRef}
                  className="absolute left-0 bottom-full mb-1 z-40 w-[260px] rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl py-1 text-[12px]"
                >
                  <div className="px-3 py-1 text-[10.5px] uppercase tracking-wider text-[var(--text-dim)]">
                    Provider
                  </div>
                  <div className="flex flex-wrap gap-1 px-2 pb-1">
                    {(Object.keys(PROVIDER_LABEL) as AgentConfig['provider'][]).map((p) => (
                      <button
                        key={p}
                        onClick={() => setProvider(p)}
                        className={`px-2 py-0.5 rounded-full border text-[11px] cursor-pointer ${
                          config.provider === p
                            ? 'border-[var(--accent)] bg-[var(--bg-active)] text-[var(--text-primary)]'
                            : 'border-[var(--border)] text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                        }`}
                      >
                        {PROVIDER_LABEL[p]}
                      </button>
                    ))}
                  </div>
                  <div className="my-1 border-t border-[var(--border)]" />
                  {modelOptions.map((m) => (
                    <div
                      key={m}
                      onClick={() => {
                        setConfig((prev) => ({ ...prev, model: m }));
                        setShowModelMenu(false);
                      }}
                      className="flex items-center gap-2 px-3 py-1.5 cursor-pointer hover:bg-[var(--bg-hover)] text-[var(--text-primary)]"
                    >
                      <span className="truncate flex-1 font-mono text-[11.5px]">{m}</span>
                      {config.model === m && <Check size={12} className="text-[var(--accent)]" />}
                    </div>
                  ))}
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
                    className="flex items-center gap-2 px-3 py-1.5 cursor-pointer hover:bg-[var(--bg-hover)] text-[var(--text-muted)]"
                  >
                    <Settings size={12} />
                    <span className="flex-1">API key & base URL…</span>
                    {!configured && (
                      <span className="text-[10.5px] text-[var(--accent)]">needs key</span>
                    )}
                  </div>
                </div>
              )}
            </div>

            {running ? (
              <button
                onClick={stopGeneration}
                className="h-7 px-2.5 text-xs font-medium rounded-md bg-[var(--danger)] text-white flex items-center gap-1.5 cursor-pointer shrink-0"
                title="Stop (Esc)"
              >
                <Square size={11} />
                Stop
              </button>
            ) : (
              <button
                onClick={sendPrompt}
                disabled={!input.trim()}
                className="h-7 px-2.5 text-xs font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] disabled:opacity-40 flex items-center gap-1.5 cursor-pointer hover:opacity-95 transition-opacity shrink-0"
                title="Send (Enter)"
              >
                <Send size={11} />
                Send
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
