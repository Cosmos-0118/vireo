import React, { Suspense, lazy, useEffect, useRef, useState } from 'react';
import {
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Play,
  RefreshCw,
  RotateCcw,
  Send,
  Settings,
  Square,
  Terminal as TerminalIcon,
  Trash2,
  Wrench,
  X,
} from 'lucide-react';
import { api, streamAgentChat } from '../api';
import { highlightLines } from '../syntax';
import type { LiteTheme } from '../themes';
import type { AgentConfig, CliToolStatus, UiChatMessage } from '../types';
import { VireoLogo } from './VireoLogo';

const CliAgentPty = lazy(() => import('./CliAgentPty'));

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

const LANG_TO_EXT: Record<string, string> = {
  ts: 'file.ts',
  typescript: 'file.ts',
  tsx: 'file.tsx',
  js: 'file.js',
  javascript: 'file.js',
  jsx: 'file.jsx',
  rs: 'file.rs',
  rust: 'file.rs',
  py: 'file.py',
  python: 'file.py',
  json: 'file.json',
  css: 'file.css',
  html: 'file.html',
  md: 'file.md',
  markdown: 'file.md',
};

const FencedCodeBlock: React.FC<{
  lang: string;
  code: string;
  theme: LiteTheme;
}> = ({ lang, code, theme }) => {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    } catch {
      // ignore clipboard errors
    }
  };

  const normalizedLang = lang.trim().toLowerCase();
  const fakePath = LANG_TO_EXT[normalizedLang];
  const isDiff = normalizedLang === 'diff';
  const highlighted = fakePath ? highlightLines(code, fakePath, theme) : null;
  const rawLines = code.split(/\r?\n/);

  return (
    <div className="my-2 rounded-md border border-[var(--border)] bg-[var(--bg-panel)] overflow-hidden">
      <div className="flex items-center justify-between px-2.5 py-1 border-b border-[var(--border)]/80 bg-[var(--bg-elevated)]/60">
        <span className="text-[10.5px] font-mono uppercase tracking-wider text-[var(--text-muted)]">
          {normalizedLang || 'code'}
        </span>
        <button
          onClick={handleCopy}
          className="flex items-center gap-1 px-1.5 py-0.5 text-[10.5px] rounded text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
          title="Copy code block"
        >
          {copied ? (
            <>
              <Check size={11} className="text-[var(--added-fg)]" />
              <span className="text-[var(--added-fg)]">Copied</span>
            </>
          ) : (
            <>
              <Copy size={11} />
              <span>Copy</span>
            </>
          )}
        </button>
      </div>
      <pre className="p-2.5 overflow-x-auto font-mono text-[12px] leading-[1.55] text-[var(--text-primary)] m-0">
        {highlighted ? (
          <code>
            {highlighted.map((lineSpans, idx) => (
              <div key={idx} className="min-h-[1.55em]">
                {lineSpans.length > 0
                  ? lineSpans.map((sp, sIdx) => (
                      <span
                        key={sIdx}
                        style={{
                          color: sp.color,
                          fontStyle: sp.italic ? 'italic' : undefined,
                          fontWeight: sp.bold ? 500 : undefined,
                        }}
                      >
                        {sp.text}
                      </span>
                    ))
                  : '\u00A0'}
              </div>
            ))}
          </code>
        ) : isDiff ? (
          <code>
            {rawLines.map((line, idx) => {
              const isAdd = line.startsWith('+') && !line.startsWith('+++');
              const isRem = line.startsWith('-') && !line.startsWith('---');
              const isHunk = line.startsWith('@@');
              return (
                <div
                  key={idx}
                  className="px-1 -mx-1"
                  style={{
                    background: isAdd
                      ? 'var(--added-bg)'
                      : isRem
                      ? 'var(--removed-bg)'
                      : undefined,
                    color: isAdd
                      ? 'var(--added-fg)'
                      : isRem
                      ? 'var(--removed-fg)'
                      : isHunk
                      ? 'var(--accent)'
                      : undefined,
                  }}
                >
                  {line || '\u00A0'}
                </div>
              );
            })}
          </code>
        ) : (
          <code>{code}</code>
        )}
      </pre>
    </div>
  );
};

function renderInlineMarkdown(text: string): React.ReactNode[] {
  // Tokenize inline code, bold, italic, and links
  const regex = /(`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|\[[^\]]+\]\((https?:\/\/[^\s)]+)\))/g;
  const parts: React.ReactNode[] = [];
  let lastIndex = 0;
  let key = 0;
  let match: RegExpExecArray | null;

  while ((match = regex.exec(text)) !== null) {
    if (match.index > lastIndex) {
      parts.push(text.slice(lastIndex, match.index));
    }
    const token = match[0];
    if (token.startsWith('`') && token.endsWith('`')) {
      parts.push(
        <code
          key={key++}
          className="px-1 py-0.5 mx-0.5 rounded bg-[var(--bg-elevated)] border border-[var(--border)]/60 font-mono text-[11.5px] text-[var(--text-primary)]"
        >
          {token.slice(1, -1)}
        </code>
      );
    } else if (token.startsWith('**') && token.endsWith('**')) {
      parts.push(
        <strong key={key++} className="font-semibold text-[var(--text-primary)]">
          {token.slice(2, -2)}
        </strong>
      );
    } else if (token.startsWith('*') && token.endsWith('*')) {
      parts.push(
        <em key={key++} className="italic">
          {token.slice(1, -1)}
        </em>
      );
    } else if (token.startsWith('[')) {
      const linkMatch = /^\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)$/.exec(token);
      if (linkMatch) {
        parts.push(
          <a
            key={key++}
            href={linkMatch[2]}
            target="_blank"
            rel="noreferrer"
            className="text-[var(--accent)] hover:underline"
          >
            {linkMatch[1]}
          </a>
        );
      } else {
        parts.push(token);
      }
    }
    lastIndex = regex.lastIndex;
  }

  if (lastIndex < text.length) {
    parts.push(text.slice(lastIndex));
  }
  return parts;
}

const MarkdownProse: React.FC<{ text: string }> = ({ text }) => {
  const lines = text.split(/\r?\n/);
  const blocks: React.ReactNode[] = [];
  let i = 0;
  let key = 0;

  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      i++;
      continue;
    }

    // Heading
    const headingMatch = /^(#{1,3})\s+(.*)$/.exec(line);
    if (headingMatch) {
      const level = headingMatch[1].length;
      const cls =
        level === 1
          ? 'text-[13.5px] font-semibold mt-2.5 mb-1 text-[var(--text-primary)]'
          : level === 2
          ? 'text-[13px] font-semibold mt-2 mb-1 text-[var(--text-primary)]'
          : 'text-[12.5px] font-semibold mt-1.5 mb-0.5 text-[var(--text-primary)]';
      blocks.push(
        <div key={key++} className={cls}>
          {renderInlineMarkdown(headingMatch[2])}
        </div>
      );
      i++;
      continue;
    }

    // Horizontal rule
    if (/^(-{3,}|\*{3,})$/.test(line.trim())) {
      blocks.push(<hr key={key++} className="my-2 border-[var(--border)]" />);
      i++;
      continue;
    }

    // Blockquote
    if (line.startsWith('>')) {
      const qLines: string[] = [];
      while (i < lines.length && lines[i].startsWith('>')) {
        qLines.push(lines[i].replace(/^>\s?/, ''));
        i++;
      }
      blocks.push(
        <blockquote
          key={key++}
          className="my-1.5 pl-2.5 border-l-2 border-[var(--accent)]/70 text-[var(--text-muted)] space-y-0.5"
        >
          {qLines.map((ql, idx) => (
            <div key={idx}>{renderInlineMarkdown(ql)}</div>
          ))}
        </blockquote>
      );
      continue;
    }

    // Unordered list
    if (/^\s*[-*]\s+/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\s*[-*]\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*[-*]\s+/, ''));
        i++;
      }
      blocks.push(
        <ul key={key++} className="my-1.5 pl-4 list-disc space-y-0.5">
          {items.map((it, idx) => (
            <li key={idx} className="leading-[1.55]">
              {renderInlineMarkdown(it)}
            </li>
          ))}
        </ul>
      );
      continue;
    }

    // Ordered list
    if (/^\s*\d+\.\s+/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*\d+\.\s+/, ''));
        i++;
      }
      blocks.push(
        <ol key={key++} className="my-1.5 pl-4 list-decimal space-y-0.5">
          {items.map((it, idx) => (
            <li key={idx} className="leading-[1.55]">
              {renderInlineMarkdown(it)}
            </li>
          ))}
        </ol>
      );
      continue;
    }

    // Regular paragraph (consume consecutive plain lines)
    const pLines: string[] = [];
    while (
      i < lines.length &&
      lines[i].trim() !== '' &&
      !/^(#{1,3}\s+|>|\s*[-*]\s+|\s*\d+\.\s+|(-{3,}|\*{3,})$)/.test(lines[i])
    ) {
      pLines.push(lines[i]);
      i++;
    }
    blocks.push(
      <p key={key++} className="my-1.5 first:mt-0 last:mb-0 leading-[1.6]">
        {pLines.map((pl, idx) => (
          <React.Fragment key={idx}>
            {idx > 0 && <br />}
            {renderInlineMarkdown(pl)}
          </React.Fragment>
        ))}
      </p>
    );
  }

  return <>{blocks}</>;
};

const MarkdownMessage: React.FC<{ content: string; theme: LiteTheme }> = ({
  content,
  theme,
}) => {
  // Split into prose and fenced code segments (tolerates unclosed fence during streaming)
  const segments: Array<
    { type: 'prose'; text: string } | { type: 'code'; lang: string; code: string }
  > = [];

  const fenceRegex = /```([a-zA-Z0-9_+-]*)[ \t]*\r?\n([\s\S]*?)(?:```|$)/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = fenceRegex.exec(content)) !== null) {
    if (match.index > lastIndex) {
      segments.push({ type: 'prose', text: content.slice(lastIndex, match.index) });
    }
    const rawCode = match[2].replace(/\r?\n$/, '');
    segments.push({ type: 'code', lang: match[1] || '', code: rawCode });
    lastIndex = fenceRegex.lastIndex;
  }

  if (lastIndex < content.length) {
    segments.push({ type: 'prose', text: content.slice(lastIndex) });
  }

  return (
    <div className="text-[13.5px] leading-[1.65] text-[var(--text-primary)] break-words">
      {segments.map((seg, idx) =>
        seg.type === 'code' ? (
          <FencedCodeBlock key={idx} lang={seg.lang} code={seg.code} theme={theme} />
        ) : (
          <MarkdownProse key={idx} text={seg.text} />
        )
      )}
    </div>
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
  const [cliMounted, setCliMounted] = useState(false);
  const [cliCmd, setCliCmd] = useState<string>('');
  const [customCliInput, setCustomCliInput] = useState<string>('');
  const [cliRunKey, setCliRunKey] = useState<number>(1);
  const [cliTools, setCliTools] = useState<CliToolStatus[]>([]);
  const [showSettings, setShowSettings] = useState(false);

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
  const abortRef = useRef<AbortController | null>(null);
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    try {
      localStorage.setItem('vireo-agent-config', JSON.stringify(config));
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

  const refreshCliTools = () => {
    api
      .cliTools()
      .then((list) => setCliTools(list))
      .catch(() => {});
  };

  useEffect(() => {
    if (mode === 'cli') {
      setCliMounted(true);
      refreshCliTools();
    }
  }, [mode]);

  const launchCli = (cmd: string) => {
    setCliCmd(cmd.trim());
    setCliRunKey((k) => k + 1);
    setCliMounted(true);
    setMode('cli');
  };

  return (
    <div className="flex flex-col h-full bg-[var(--bg-sidebar)] border-l border-[var(--border)]">
      {/* Header */}
      <div className="flex items-center justify-between px-3 h-10 border-b border-[var(--border)] shrink-0">
        <div className="flex items-center gap-1.5">
          <button
            onClick={() => setMode('chat')}
            className={`px-2.5 py-1 text-[12.5px] rounded-md flex items-center gap-1.5 cursor-pointer transition-colors ${
              mode === 'chat'
                ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
            }`}
          >
            <Bot size={14} />
            Agent Chat
          </button>
          <button
            onClick={() => setMode('cli')}
            className={`px-2.5 py-1 text-[12.5px] rounded-md flex items-center gap-1.5 cursor-pointer transition-colors ${
              mode === 'cli'
                ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
            }`}
          >
            <TerminalIcon size={14} />
            CLI Agent
          </button>
        </div>

        <div className="flex items-center gap-1">
          {mode === 'chat' && (
            <>
              <button
                onClick={() => setMessages([])}
                className="p-1.5 text-[var(--text-muted)] hover:text-[var(--text-primary)] rounded-md hover:bg-[var(--bg-hover)] cursor-pointer"
                title="Clear chat"
              >
                <Trash2 size={14} />
              </button>
              <button
                onClick={() => setShowSettings(!showSettings)}
                className={`p-1.5 rounded-md hover:bg-[var(--bg-hover)] cursor-pointer ${
                  showSettings
                    ? 'text-[var(--accent)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                }`}
                title="Agent model & API settings"
              >
                <Settings size={15} />
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
                Base URL (optional, for OpenRouter, DeepSeek, or LM Studio)
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

      {/* Agent Chat View (kept separate from persistent CLI PTY) */}
      <div className={mode === 'chat' ? 'flex-1 flex flex-col min-h-0' : 'hidden'}>
          {/* Chat timeline */}
          <div className="flex-1 overflow-y-auto p-3.5 space-y-3.5">
            {messages.length === 0 ? (
              <div className="space-y-4 pt-5 text-[13px] text-[var(--text-muted)]">
                <div className="flex items-center gap-3">
                  <VireoLogo size={36} className="shrink-0 drop-shadow-sm" />
                  <div>
                    <div className="text-[15px] font-semibold tracking-tight text-[var(--text-primary)]">
                      Vireo Agent
                    </div>
                    <div className="text-xs text-[var(--text-dim)]">
                      Live diffs & automatic Git checkpoints
                    </div>
                  </div>
                </div>
                <p className="leading-relaxed">
                  Ask the agent to inspect files, implement features, or fix terminal errors. Every
                  file edit streams live into the Diff Viewer with automatic Git checkpoints.
                </p>
                <div className="p-3.5 rounded-lg border border-[var(--border)] bg-[var(--bg-base)] space-y-2">
                  <div className="font-medium text-[var(--text-primary)]">Two ways to code:</div>
                  <div className="leading-relaxed">
                    1. Click <Settings size={13} className="inline -mt-0.5 mx-0.5" /> above to connect an LLM API
                    (OpenAI, Claude, Gemini, OpenRouter, or local Ollama).
                  </div>
                  <div className="leading-relaxed">
                    2. Or switch to{' '}
                    <button
                      onClick={() => setMode('cli')}
                      className="underline text-[var(--accent)] font-medium cursor-pointer"
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
                    <MarkdownMessage content={m.content} theme={theme} />
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
          <div className="p-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)] shrink-0">
            <div className="rounded-lg border border-[var(--border)] bg-[var(--bg-input)] focus-within:border-[var(--accent)] transition-colors">
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
                className="w-full p-3 text-[13px] leading-relaxed bg-transparent text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none resize-none"
              />
              <div className="flex items-center justify-between px-3 py-2 border-t border-[var(--border)]/60">
                <span className="text-[11.5px] text-[var(--text-dim)] truncate">
                  {config.provider} · {config.model}
                </span>
                {running ? (
                  <button
                    onClick={stopGeneration}
                    className="px-3 py-1.5 text-xs font-medium rounded-md bg-[var(--danger)] text-white flex items-center gap-1.5 cursor-pointer"
                  >
                    <Square size={12} />
                    Stop
                  </button>
                ) : (
                  <button
                    onClick={sendPrompt}
                    disabled={!input.trim()}
                    className="px-3 py-1.5 text-xs font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] disabled:opacity-40 flex items-center gap-1.5 cursor-pointer hover:opacity-95 transition-opacity"
                  >
                    <Send size={12} />
                    Send
                  </button>
                )}
              </div>
            </div>
          </div>
      </div>

      {/* Persistent CLI Agent View (lazy-mounted on first visit, preserved across tab switches) */}
      {cliMounted && (
        <div className={mode === 'cli' ? 'flex-1 flex flex-col min-h-0' : 'hidden'}>
          <div className="flex items-center justify-between gap-1 px-2 py-1.5 border-b border-[var(--border)] shrink-0">
            <div className="flex items-center gap-1 overflow-x-auto min-w-0">
              <span className="text-[10px] text-[var(--text-dim)] shrink-0 mr-1">Launch:</span>
              {[
                { label: 'Shell', cmd: '' },
                { label: 'Claude', cmd: 'claude' },
                { label: 'Codex', cmd: 'codex' },
                { label: 'Gemini', cmd: 'gemini' },
                { label: 'Aider', cmd: 'aider' },
                { label: 'OpenCode', cmd: 'opencode' },
              ].map((item) => {
                const status = item.cmd
                  ? cliTools.find((t) => t.cmd === item.cmd)
                  : undefined;
                const isActive = cliCmd === item.cmd;
                const title = !item.cmd
                  ? 'Interactive shell in workspace'
                  : status?.installed
                  ? `Launch ${item.label} (${status.path})`
                  : status
                  ? `${item.label} was not found on PATH. Click for install instructions (${status.install_hint})`
                  : `Launch ${item.label}`;
                return (
                  <button
                    key={item.label}
                    onClick={(e) => {
                      e.currentTarget.blur();
                      launchCli(item.cmd);
                    }}
                    title={title}
                    className={`px-2 py-0.5 text-[11px] rounded border shrink-0 cursor-pointer flex items-center gap-1 transition-colors ${
                      isActive
                        ? 'border-[var(--accent)] bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                        : 'border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)]'
                    }`}
                  >
                    {status && (
                      <span
                        className="w-1.5 h-1.5 rounded-full shrink-0"
                        style={{
                          background: status.installed ? '#10b981' : 'var(--text-dim)',
                          opacity: status.installed ? 1 : 0.45,
                        }}
                      />
                    )}
                    {item.label}
                  </button>
                );
              })}
            </div>
            <button
              onClick={(e) => {
                e.currentTarget.blur();
                refreshCliTools();
                launchCli(cliCmd);
              }}
              className="p-1 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] rounded shrink-0 cursor-pointer"
              title="Restart current CLI session"
            >
              <RefreshCw size={12} />
            </button>
          </div>

          <form
            onSubmit={(e) => {
              e.preventDefault();
              const trimmed = customCliInput.trim();
              if (!trimmed) return;
              launchCli(trimmed);
            }}
            className="flex items-center gap-1.5 px-2 py-1 border-b border-[var(--border)] bg-[var(--bg-panel)]/60 shrink-0"
          >
            <input
              value={customCliInput}
              onChange={(e) => setCustomCliInput(e.target.value)}
              placeholder="Custom CLI command (e.g. claude --resume, codex --full-auto)..."
              className="flex-1 min-w-0 px-2 py-0.5 text-[11.5px] font-mono rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none focus:border-[var(--accent)]"
            />
            <button
              type="submit"
              disabled={!customCliInput.trim()}
              className="px-2 py-0.5 text-[11px] rounded bg-[var(--bg-elevated)] border border-[var(--border)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] disabled:opacity-40 flex items-center gap-1 shrink-0 cursor-pointer"
              title="Run custom CLI command"
            >
              <Play size={10} />
              Run
            </button>
          </form>

          <Suspense
            fallback={
              <div className="flex-1 flex items-center justify-center text-xs text-[var(--text-dim)]">
                Starting CLI terminal...
              </div>
            }
          >
            <CliAgentPty
              command={cliCmd || undefined}
              runKey={cliRunKey}
              active={mode === 'cli'}
              theme={theme}
              workspacePath={workspacePath}
            />
          </Suspense>
        </div>
      )}
    </div>
  );
};
