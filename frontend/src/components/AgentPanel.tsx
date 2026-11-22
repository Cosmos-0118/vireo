import React, { useEffect, useRef, useState } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import {
  Bot,
  ChevronDown,
  ChevronRight,
  RotateCcw,
  Send,
  Settings,
  Square,
  Terminal as TerminalIcon,
  Trash2,
  Wrench,
  X,
} from 'lucide-react';
import { api, streamAgentChat, wsUrl } from '../api';
import type { LiteTheme } from '../themes';
import type { AgentConfig, UiChatMessage } from '../types';

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
};

const CliAgentPty: React.FC<{
  command?: string;
  runKey: number;
  theme: LiteTheme;
  workspacePath: string;
}> = ({ command, runKey, theme, workspacePath }) => {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);

  useEffect(() => {
    if (!hostRef.current) return;
    const term = new Terminal({
      cursorBlink: true,
      fontSize: 12.5,
      fontFamily: "'Cascadia Code', 'JetBrains Mono', Consolas, monospace",
      theme: theme.terminal,
      scrollback: 3000,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(hostRef.current);
    termRef.current = term;

    const params = new URLSearchParams({ cols: '80', rows: '30' });
    if (command) params.set('cmd', command);

    const sock = new WebSocket(wsUrl(`/ws/pty?${params.toString()}`));

    const resize = () => {
      try {
        fit.fit();
        if (sock.readyState === WebSocket.OPEN) {
          sock.send(JSON.stringify({ type: 'resize', cols: term.cols, rows: term.rows }));
        }
      } catch {
        // ignore
      }
    };

    sock.onopen = resize;
    sock.onmessage = (e) => {
      if (typeof e.data === 'string') term.write(e.data);
    };
    sock.onclose = () => term.write('\r\n\x1b[90m[Agent session ended]\x1b[0m\r\n');

    const sub = term.onData((data) => {
      if (sock.readyState === WebSocket.OPEN) {
        sock.send(JSON.stringify({ type: 'input', data }));
      }
    });

    const ro = new ResizeObserver(resize);
    ro.observe(hostRef.current);
    setTimeout(resize, 50);

    return () => {
      ro.disconnect();
      sub.dispose();
      sock.close();
      term.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [runKey, workspacePath]);

  useEffect(() => {
    if (termRef.current) {
      termRef.current.options.theme = theme.terminal;
    }
  }, [theme]);

  return (
    <div
      ref={hostRef}
      className="flex-1 min-h-0"
      style={{ background: theme.terminal.background }}
    />
  );
};

interface AgentPanelProps {
  theme: LiteTheme;
  workspacePath: string;
  draftAppend: { id: number; text: string } | null;
  onOpenDiff: (path: string) => void;
  onRefreshGit: () => Promise<void>;
  onToast: (msg: string, isError?: boolean) => void;
}

export const AgentPanel: React.FC<AgentPanelProps> = ({
  theme,
  workspacePath,
  draftAppend,
  onOpenDiff,
  onRefreshGit,
  onToast,
}) => {
  const [mode, setMode] = useState<'chat' | 'cli'>('chat');
  const [cliCmd, setCliCmd] = useState<string>('');
  const [cliRunKey, setCliRunKey] = useState<number>(1);
  const [showSettings, setShowSettings] = useState(false);

  const [config, setConfig] = useState<AgentConfig>(() => {
    try {
      const saved = localStorage.getItem('litecode-agent-config');
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
  const abortRef = useRef<AbortController | null>(null);
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    try {
      localStorage.setItem('litecode-agent-config', JSON.stringify(config));
    } catch {
      // ignore
    }
  }, [config]);

  useEffect(() => {
    if (draftAppend?.text) {
      setMode('chat');
      setInput((prev) => (prev ? `${prev}\n\n${draftAppend.text}` : draftAppend.text));
    }
  }, [draftAppend]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

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
      { role: 'user', content: trimmed },
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
            m.id === assistantId
              ? { ...m, error: String(e), streaming: false }
              : m
          )
        );
      }
      setRunning(false);
    }
  };

  const stopGeneration = () => {
    abortRef.current?.abort();
    setRunning(false);
  };

  const launchCli = (cmd: string) => {
    setCliCmd(cmd);
    setCliRunKey((k) => k + 1);
    setMode('cli');
  };

  return (
    <div className="flex flex-col h-full bg-[var(--bg-sidebar)] border-l border-[var(--border)]">
      {/* Header */}
      <div className="flex items-center justify-between px-2.5 h-9 border-b border-[var(--border)] shrink-0">
        <div className="flex items-center gap-1">
          <button
            onClick={() => setMode('chat')}
            className={`px-2 py-1 text-xs rounded flex items-center gap-1 cursor-pointer ${
              mode === 'chat'
                ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
            }`}
          >
            <Bot size={13} />
            Agent Chat
          </button>
          <button
            onClick={() => setMode('cli')}
            className={`px-2 py-1 text-xs rounded flex items-center gap-1 cursor-pointer ${
              mode === 'cli'
                ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
            }`}
          >
            <TerminalIcon size={13} />
            CLI Agent
          </button>
        </div>

        <div className="flex items-center gap-1">
          {mode === 'chat' && (
            <>
              <button
                onClick={() => setMessages([])}
                className="p-1 text-[var(--text-muted)] hover:text-[var(--text-primary)] rounded hover:bg-[var(--bg-hover)] cursor-pointer"
                title="Clear chat"
              >
                <Trash2 size={13} />
              </button>
              <button
                onClick={() => setShowSettings(!showSettings)}
                className={`p-1 rounded hover:bg-[var(--bg-hover)] cursor-pointer ${
                  showSettings
                    ? 'text-[var(--accent)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                }`}
                title="Agent model & API settings"
              >
                <Settings size={14} />
              </button>
            </>
          )}
        </div>
      </div>

      {/* Settings Drawer */}
      {showSettings && mode === 'chat' && (
        <div className="p-3 border-b border-[var(--border)] bg-[var(--bg-elevated)] space-y-2 text-xs shrink-0">
          <div className="flex items-center justify-between font-semibold text-[var(--text-primary)]">
            <span>Agent Model Settings</span>
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
                onChange={(e) => {
                  const p = e.target.value as AgentConfig['provider'];
                  setConfig((prev) => ({
                    ...prev,
                    provider: p,
                    model: PROVIDER_PRESETS[p].model,
                    baseUrl: PROVIDER_PRESETS[p].baseUrl,
                  }));
                }}
                className="w-full px-2 py-1 rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none"
              >
                <option value="openai">OpenAI / Compatible</option>
                <option value="anthropic">Anthropic Claude</option>
                <option value="gemini">Google Gemini</option>
                <option value="ollama">Ollama (Local)</option>
              </select>
            </div>
            <div>
              <label className="block text-[11px] text-[var(--text-muted)] mb-0.5">Model</label>
              <input
                value={config.model}
                onChange={(e) => setConfig((prev) => ({ ...prev, model: e.target.value }))}
                className="w-full px-2 py-1 rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none"
              />
            </div>
          </div>

          {config.provider !== 'gemini' && (
            <div>
              <label className="block text-[11px] text-[var(--text-muted)] mb-0.5">
                Base URL (optional — works with OpenRouter, DeepSeek, LM Studio)
              </label>
              <input
                value={config.baseUrl}
                onChange={(e) => setConfig((prev) => ({ ...prev, baseUrl: e.target.value }))}
                placeholder="https://api.openai.com/v1"
                className="w-full px-2 py-1 rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none"
              />
            </div>
          )}

          {config.provider !== 'ollama' && (
            <div>
              <label className="block text-[11px] text-[var(--text-muted)] mb-0.5">
                API Key (saved only in local browser storage)
              </label>
              <input
                type="password"
                value={config.apiKey}
                onChange={(e) => setConfig((prev) => ({ ...prev, apiKey: e.target.value }))}
                placeholder="sk-..."
                className="w-full px-2 py-1 rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none"
              />
            </div>
          )}

          <label className="flex items-center gap-2 text-[11px] text-[var(--text-muted)] cursor-pointer">
            <input
              type="checkbox"
              checked={config.autoCheckpoint}
              onChange={(e) =>
                setConfig((prev) => ({ ...prev, autoCheckpoint: e.target.checked }))
              }
            />
            Auto-create Git checkpoint before agent edits
          </label>
        </div>
      )}

      {mode === 'chat' ? (
        <>
          {/* Chat timeline */}
          <div className="flex-1 overflow-y-auto p-3 space-y-3">
            {messages.length === 0 ? (
              <div className="space-y-3 pt-4 text-xs text-[var(--text-muted)]">
                <div className="text-sm font-medium text-[var(--text-primary)]">
                  LiteCode Agent
                </div>
                <p className="leading-relaxed">
                  Ask the agent to inspect files, implement features, or fix terminal errors. Every
                  file edit streams live into the Diff Viewer with automatic Git checkpoints.
                </p>
                <div className="p-2.5 rounded border border-[var(--border)] bg-[var(--bg-base)] space-y-1.5">
                  <div className="font-medium text-[var(--text-primary)]">Two ways to code:</div>
                  <div>
                    1. Click <Settings size={11} className="inline" /> above to connect an LLM API
                    (OpenAI, Claude, Gemini, OpenRouter, or local Ollama).
                  </div>
                  <div>
                    2. Or switch to{' '}
                    <button
                      onClick={() => setMode('cli')}
                      className="underline text-[var(--accent)] cursor-pointer"
                    >
                      CLI Agent
                    </button>{' '}
                    to run Claude Code, Codex, Gemini CLI, or Aider side-by-side while watching live
                    diffs.
                  </div>
                </div>
              </div>
            ) : (
              messages.map((m) => (
                <div
                  key={m.id}
                  className={`rounded-md p-2.5 text-xs ${
                    m.role === 'user'
                      ? 'bg-[var(--bg-elevated)] border border-[var(--border)] ml-4'
                      : 'bg-[var(--bg-base)] border border-[var(--border)]'
                  }`}
                >
                  <div className="flex items-center justify-between mb-1 text-[10px] font-semibold uppercase tracking-wide text-[var(--text-dim)]">
                    <span>{m.role === 'user' ? 'You' : `Agent (${config.model})`}</span>
                    {m.streaming && (
                      <span className="text-[var(--accent)] animate-pulse">working...</span>
                    )}
                  </div>

                  {m.checkpoint && (
                    <div className="mb-2 flex items-center justify-between px-2 py-1 rounded bg-[var(--bg-elevated)] border border-[var(--border)] text-[11px]">
                      <span className="text-[var(--text-muted)]">
                        Checkpoint saved ({m.checkpoint.created_at})
                      </span>
                      <button
                        onClick={async () => {
                          const res = await api.gitRollback(m.checkpoint!.id);
                          if (res.ok) {
                            onToast('Rolled back agent changes!');
                            onRefreshGit();
                          } else {
                            onToast(res.stderr || 'Rollback failed', true);
                          }
                        }}
                        className="flex items-center gap-1 text-[var(--accent)] hover:underline cursor-pointer"
                      >
                        <RotateCcw size={11} />
                        Undo Turn
                      </button>
                    </div>
                  )}

                  {m.tools && m.tools.length > 0 && (
                    <div className="space-y-1 mb-2">
                      {m.tools.map((t) => {
                        const open = !!expandedTools[t.id];
                        const filePath =
                          typeof t.args?.path === 'string' ? t.args.path : undefined;
                        return (
                          <div
                            key={t.id}
                            className="rounded border border-[var(--border)] bg-[var(--bg-sidebar)] overflow-hidden"
                          >
                            <div
                              onClick={() =>
                                setExpandedTools((prev) => ({ ...prev, [t.id]: !open }))
                              }
                              className="flex items-center justify-between px-2 py-1 cursor-pointer hover:bg-[var(--bg-hover)]"
                            >
                              <div className="flex items-center gap-1.5 min-w-0">
                                {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
                                <Wrench size={11} className="text-[var(--accent)] shrink-0" />
                                <span className="font-mono font-medium">{t.name}</span>
                                {filePath && (
                                  <button
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      onOpenDiff(filePath);
                                    }}
                                    className="font-mono text-[var(--accent)] hover:underline truncate cursor-pointer"
                                  >
                                    {filePath}
                                  </button>
                                )}
                              </div>
                              <span className="text-[10px] text-[var(--text-dim)] shrink-0">
                                {t.result !== undefined ? 'done' : 'running...'}
                              </span>
                            </div>
                            {open && (
                              <pre className="p-2 text-[11px] font-mono overflow-x-auto border-t border-[var(--border)] text-[var(--text-muted)] max-h-40 whitespace-pre-wrap">
                                {JSON.stringify(t.args, null, 2)}
                                {t.result ? `\n---\n${t.result}` : ''}
                              </pre>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  )}

                  {m.content && (
                    <div className="whitespace-pre-wrap leading-relaxed text-[var(--text-primary)] break-words">
                      {m.content}
                    </div>
                  )}

                  {m.error && (
                    <div className="mt-2 p-2 rounded bg-[var(--removed-bg)] text-[var(--removed-fg)] text-xs">
                      {m.error}
                    </div>
                  )}
                </div>
              ))
            )}
            <div ref={endRef} />
          </div>

          {/* Prompt input */}
          <div className="p-2.5 border-t border-[var(--border)] bg-[var(--bg-sidebar)] shrink-0">
            <div className="rounded border border-[var(--border)] bg-[var(--bg-input)] focus-within:border-[var(--accent)]">
              <textarea
                value={input}
                onChange={(e) => setInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault();
                    sendPrompt();
                  }
                }}
                rows={3}
                placeholder="Ask the agent to edit code, explain a diff, or fix an error... (Enter to send)"
                className="w-full p-2 text-xs bg-transparent text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none resize-none"
              />
              <div className="flex items-center justify-between px-2 py-1.5 border-t border-[var(--border)]/60">
                <span className="text-[10px] text-[var(--text-dim)] truncate">
                  {config.provider} · {config.model}
                </span>
                {running ? (
                  <button
                    onClick={stopGeneration}
                    className="px-2.5 py-1 text-xs rounded bg-[var(--danger)] text-white flex items-center gap-1 cursor-pointer"
                  >
                    <Square size={11} />
                    Stop
                  </button>
                ) : (
                  <button
                    onClick={sendPrompt}
                    disabled={!input.trim()}
                    className="px-2.5 py-1 text-xs rounded bg-[var(--accent)] text-[var(--accent-fg)] disabled:opacity-40 flex items-center gap-1 cursor-pointer"
                  >
                    <Send size={11} />
                    Send
                  </button>
                )}
              </div>
            </div>
          </div>
        </>
      ) : (
        <div className="flex-1 flex flex-col min-h-0">
          <div className="flex items-center gap-1 px-2 py-1.5 border-b border-[var(--border)] overflow-x-auto shrink-0">
            <span className="text-[10px] text-[var(--text-dim)] shrink-0 mr-1">Launch:</span>
            {[
              { label: 'Shell', cmd: '' },
              { label: 'Claude', cmd: 'claude' },
              { label: 'Codex', cmd: 'codex' },
              { label: 'Gemini', cmd: 'gemini' },
              { label: 'Aider', cmd: 'aider' },
              { label: 'OpenCode', cmd: 'opencode' },
            ].map((item) => (
              <button
                key={item.label}
                onClick={() => launchCli(item.cmd)}
                className="px-2 py-0.5 text-[11px] rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] shrink-0 cursor-pointer"
              >
                {item.label}
              </button>
            ))}
          </div>
          <CliAgentPty
            command={cliCmd || undefined}
            runKey={cliRunKey}
            theme={theme}
            workspacePath={workspacePath}
          />
        </div>
      )}
    </div>
  );
};
