import React, { useEffect, useLayoutEffect, useRef, useState } from 'react';
import {
  Check,
  ChevronDown,
  Copy,
  Maximize2,
  Minimize2,
  Play,
  Plus,
  RotateCw,
  Send,
  Sparkles,
  Trash2,
  X,
  Terminal as TerminalIcon,
} from 'lucide-react';
import { api } from '../api';
import type { LiteTheme } from '../themes';
import type { CliToolStatus } from '../types';
import {
  killPtySession,
  readTerminalTail,
  usePtyTerminal,
  useTerminalEngine,
} from './usePtyTerminal';

export interface TermTabSpec {
  id: string;
  title: string;
  cmd?: string;
  /** 'agent' tabs host a CLI coding agent (claude, codex, ...) and can be restarted. */
  kind?: 'shell' | 'agent';
  /** Bumped to restart the tab's process (new backend session). */
  runKey?: number;
}

const ptySid = (workspace: string, tab: TermTabSpec) =>
  `term::${workspace}::${tab.id}::${tab.runKey ?? 0}::${tab.cmd ?? ''}`;

const AGENT_PRESETS: { id: string; label: string; cmd: string }[] = [
  { id: 'claude', label: 'Claude Code', cmd: 'claude' },
  { id: 'codex', label: 'Codex CLI', cmd: 'codex' },
  { id: 'gemini', label: 'Gemini CLI', cmd: 'gemini' },
  { id: 'aider', label: 'Aider', cmd: 'aider' },
  { id: 'opencode', label: 'OpenCode', cmd: 'opencode' },
];

interface SingleTerminalProps {
  tab: TermTabSpec;
  active: boolean;
  theme: LiteTheme;
  workspaceKey: string;
  registerReader: (id: string, reader: (() => string) | null) => void;
}

const SingleTerminal: React.FC<SingleTerminalProps> = ({
  tab,
  active,
  theme,
  workspaceKey,
  registerReader,
}) => {
  const hostRef = useRef<HTMLDivElement>(null);
  const isAgent = tab.kind === 'agent';
  const { termRef } = usePtyTerminal(hostRef, {
    sessionKey: ptySid(workspaceKey, tab),
    cmd: tab.cmd,
    active,
    theme,
    fontSize: 14,
    scrollback: isAgent ? 5000 : 3000,
    exitMessage: isAgent ? '[Agent session ended]' : '[Process exited]',
    focusOnOpen: isAgent,
  });

  useEffect(() => {
    registerReader(tab.id, () => (termRef.current ? readTerminalTail(termRef.current) : ''));
    return () => registerReader(tab.id, null);
  }, [tab.id, registerReader, termRef]);

  return (
    <div
      className="h-full w-full box-border"
      style={{
        display: active ? 'block' : 'none',
        background: theme.terminal.background,
        padding: '8px 10px',
      }}
      onClick={() => termRef.current?.focus()}
    >
      <div ref={hostRef} className="h-full w-full" />
    </div>
  );
};

interface LauncherMenuProps {
  anchor: DOMRect;
  tools: CliToolStatus[];
  onLaunch: (label: string, cmd: string) => void;
  onClose: () => void;
}

const LauncherMenu: React.FC<LauncherMenuProps> = ({ anchor, tools, onLaunch, onClose }) => {
  const [custom, setCustom] = useState('');
  const [copied, setCopied] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; bottom: number }>({
    left: anchor.left,
    bottom: window.innerHeight - anchor.top + 4,
  });

  useLayoutEffect(() => {
    const w = ref.current?.offsetWidth ?? 320;
    setPos({
      left: Math.max(8, Math.min(anchor.left, window.innerWidth - w - 8)),
      bottom: window.innerHeight - anchor.top + 4,
    });
  }, [anchor]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const target = e.target as Element;
      if (target.closest?.('[data-launcher-btn]')) return; // the button toggles the menu itself
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
  }, [onClose]);

  const copyHint = async (id: string, hint: string) => {
    try {
      await navigator.clipboard.writeText(hint);
      setCopied(id);
      setTimeout(() => setCopied(null), 1600);
    } catch {
      // ignore clipboard errors
    }
  };

  return (
    <div
      ref={ref}
      style={{ position: 'fixed', left: pos.left, bottom: pos.bottom, zIndex: 60, width: 320 }}
      className="rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl py-1 text-[12.5px]"
    >
      <div className="px-3 py-1.5 text-[10.5px] uppercase tracking-wider text-[var(--text-dim)]">
        Launch CLI agent
      </div>
      {AGENT_PRESETS.map((p) => {
        const status = tools.find((t) => t.cmd === p.cmd);
        const installed = status?.installed;
        return (
          <div
            key={p.id}
            onClick={() => onLaunch(p.label, p.cmd)}
            className="flex items-center gap-2 px-3 py-1.5 cursor-pointer hover:bg-[var(--bg-hover)]"
            title={
              installed
                ? status?.path || p.cmd
                : status
                ? `${p.cmd} was not found on PATH`
                : `Launch ${p.label}`
            }
          >
            <span
              className="w-1.5 h-1.5 rounded-full shrink-0"
              style={{
                background: installed ? '#10b981' : 'var(--text-dim)',
                opacity: installed ? 1 : 0.45,
              }}
            />
            <span className="text-[var(--text-primary)] font-medium">{p.label}</span>
            <span className="font-mono text-[11px] text-[var(--text-dim)] truncate min-w-0 flex-1">
              {installed ? status?.path || '' : ''}
            </span>
            {status && !installed && status.install_hint && (
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  copyHint(p.id, status.install_hint);
                }}
                className="flex items-center gap-1 px-1.5 py-0.5 text-[10.5px] rounded border border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-active)] cursor-pointer shrink-0"
                title={status.install_hint}
              >
                {copied === p.id ? <Check size={10} /> : <Copy size={10} />}
                {copied === p.id ? 'Copied' : 'Install cmd'}
              </button>
            )}
          </div>
        );
      })}
      <div className="my-1 border-t border-[var(--border)]" />
      <form
        onSubmit={(e) => {
          e.preventDefault();
          const c = custom.trim();
          if (c) onLaunch(c.split(/\s+/)[0], c);
        }}
        className="flex items-center gap-1.5 px-3 py-1.5"
      >
        <input
          value={custom}
          onChange={(e) => setCustom(e.target.value)}
          placeholder="Custom command (e.g. claude --resume)"
          className="flex-1 min-w-0 px-2 py-1 text-[11.5px] font-mono rounded bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none focus:border-[var(--accent)]"
        />
        <button
          type="submit"
          disabled={!custom.trim()}
          className="p-1.5 rounded bg-[var(--bg-active)] border border-[var(--border)] text-[var(--text-primary)] disabled:opacity-40 cursor-pointer"
          title="Run custom command"
        >
          <Play size={11} />
        </button>
      </form>
    </div>
  );
};

interface TerminalPanelProps {
  theme: LiteTheme;
  workspacePath: string;
  pendingCommand: { id: number; name: string; command: string } | null;
  /** Increment to open the "Launch CLI agent" menu (e.g. from the Agent Chat header). */
  launcherSignal?: number;
  maximized?: boolean;
  onToggleMaximize?: () => void;
  onSendOutputToAgent: (output: string) => void;
  onClosePanel?: () => void;
}

export const TerminalPanel: React.FC<TerminalPanelProps> = ({
  theme,
  workspacePath,
  pendingCommand,
  launcherSignal = 0,
  maximized = false,
  onToggleMaximize,
  onSendOutputToAgent,
  onClosePanel,
}) => {
  const [tabs, setTabs] = useState<TermTabSpec[]>([
    { id: 'shell-1', title: 'PowerShell', kind: 'shell' },
  ]);
  const [activeId, setActiveId] = useState<string>('shell-1');
  const [engine, setEngine] = useTerminalEngine();
  const [cliTools, setCliTools] = useState<CliToolStatus[]>([]);
  const [menuAnchor, setMenuAnchor] = useState<DOMRect | null>(null);
  const launcherBtnRef = useRef<HTMLButtonElement>(null);
  const readersRef = useRef<Record<string, () => string>>({});
  const registerReader = React.useCallback((id: string, reader: (() => string) | null) => {
    if (reader) readersRef.current[id] = reader;
    else delete readersRef.current[id];
  }, []);

  useEffect(() => {
    if (!pendingCommand) return;
    const id = `task-${pendingCommand.id}`;
    setTabs((prev) => [
      ...prev,
      { id, title: pendingCommand.name, cmd: pendingCommand.command, kind: 'shell' },
    ]);
    setActiveId(id);
  }, [pendingCommand]);

  const openMenu = () => {
    const rect = launcherBtnRef.current?.getBoundingClientRect();
    if (!rect) return;
    api
      .cliTools()
      .then((list) => setCliTools(list))
      .catch(() => {});
    setMenuAnchor(rect);
  };

  useEffect(() => {
    if (launcherSignal > 0) openMenu();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [launcherSignal]);

  const addShell = () => {
    const id = `shell-${Date.now()}`;
    setTabs((prev) => [...prev, { id, title: `Shell ${prev.length + 1}`, kind: 'shell' }]);
    setActiveId(id);
  };

  const launchAgent = (label: string, cmd: string) => {
    const id = `agent-${Date.now()}`;
    setTabs((prev) => [...prev, { id, title: label, cmd, kind: 'agent', runKey: 1 }]);
    setActiveId(id);
    setMenuAnchor(null);
  };

  const restartTab = (id: string) => {
    const t = tabs.find((x) => x.id === id);
    if (!t) return;
    killPtySession(ptySid(workspacePath, t));
    setTabs((prev) => prev.map((x) => (x.id === id ? { ...x, runKey: (x.runKey ?? 0) + 1 } : x)));
  };

  const closeTab = (id: string) => {
    const closing = tabs.find((t) => t.id === id);
    if (closing) killPtySession(ptySid(workspacePath, closing));
    setTabs((prev) => {
      const next = prev.filter((t) => t.id !== id);
      if (next.length === 0) {
        const fresh: TermTabSpec = { id: `shell-${Date.now()}`, title: 'PowerShell', kind: 'shell' };
        setActiveId(fresh.id);
        return [fresh];
      }
      if (activeId === id) {
        setActiveId(next[next.length - 1].id);
      }
      return next;
    });
  };

  const sendLastOutput = () => {
    const tail = (readersRef.current[activeId]?.() || '').trim();
    if (!tail) return;
    const excerpt = tail.slice(-2800);
    onSendOutputToAgent(
      `Here is the latest terminal output. Help diagnose/fix any errors:\n\`\`\`\n${excerpt}\n\`\`\`\n`
    );
  };

  const activeTab = tabs.find((t) => t.id === activeId);

  return (
    <div className="flex flex-col h-full bg-[var(--bg-panel)] border-t border-[var(--border)]">
      {/* Terminal tab bar */}
      <div className="@container flex items-center justify-between gap-2 px-3 h-9 border-b border-[var(--border)] bg-[var(--bg-sidebar)] shrink-0">
        <div className="flex items-center gap-1.5 min-w-0 flex-1 overflow-x-auto">
          {tabs.map((t) => (
            <div
              key={t.id}
              onClick={() => setActiveId(t.id)}
              className={`flex items-center gap-1.5 min-w-0 whitespace-nowrap px-3 py-1 text-[12.5px] rounded-md cursor-pointer select-none transition-colors ${
                t.id === activeId
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                  : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
              }`}
            >
              {t.kind === 'agent' ? (
                <Sparkles size={13} className="text-[var(--accent)]" />
              ) : (
                <TerminalIcon size={13} />
              )}
              <span className="max-w-[160px] truncate">{t.title}</span>
              {tabs.length > 1 && (
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    closeTab(t.id);
                  }}
                  className="hover:text-[var(--danger)] cursor-pointer"
                >
                  <X size={12} />
                </button>
              )}
            </div>
          ))}
          <button
            onClick={addShell}
            className="shrink-0 p-1.5 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] rounded-md cursor-pointer"
            title="New terminal"
          >
            <Plus size={15} />
          </button>
          <button
            ref={launcherBtnRef}
            data-launcher-btn
            onClick={() => (menuAnchor ? setMenuAnchor(null) : openMenu())}
            className={`shrink-0 flex items-center gap-1 px-2 py-1 text-[12px] rounded-md cursor-pointer whitespace-nowrap transition-colors ${
              menuAnchor
                ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
            }`}
            title="Launch a CLI agent (Claude Code, Codex, Gemini, Aider, OpenCode…)"
          >
            <Sparkles size={13} />
            <span className="hidden @[520px]:inline">Launch Agent</span>
            <ChevronDown size={12} />
          </button>
        </div>

        <div className="flex items-center gap-1.5 shrink-0">
          {activeTab?.kind === 'agent' && (
            <button
              onClick={() => restartTab(activeTab.id)}
              className="p-1.5 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] rounded-md cursor-pointer"
              title="Restart agent"
            >
              <RotateCw size={14} />
            </button>
          )}
          <button
            onClick={() => setEngine(engine === 'xterm' ? 'ghostty' : 'xterm')}
            className="px-2 py-1 text-[11px] font-mono whitespace-nowrap rounded-md border border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
            title={`Terminal engine: ${engine === 'xterm' ? 'xterm.js + WebGL' : 'Ghostty (WASM)'}. Click to switch; running shells are kept.`}
          >
            {engine === 'xterm' ? 'xterm' : 'ghostty'}
          </button>
          <button
            onClick={sendLastOutput}
            className="px-2.5 py-1 text-xs whitespace-nowrap rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] flex items-center gap-1.5 cursor-pointer transition-colors"
            title="Send recent terminal output to Agent Chat"
          >
            <Send size={12} />
            <span className="hidden @[620px]:inline">Send Output to Agent</span>
          </button>
          <button
            onClick={() => closeTab(activeId)}
            className="p-1.5 text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)] rounded-md cursor-pointer"
            title="Kill current terminal"
          >
            <Trash2 size={14} />
          </button>
          {onToggleMaximize && (
            <button
              onClick={onToggleMaximize}
              className="p-1.5 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] rounded-md cursor-pointer"
              title={maximized ? 'Restore terminal size' : 'Maximize terminal'}
            >
              {maximized ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
            </button>
          )}
          {onClosePanel && (
            <button
              onClick={onClosePanel}
              className="p-1.5 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] rounded-md cursor-pointer"
              title="Hide terminal (Ctrl+`)"
            >
              <X size={15} />
            </button>
          )}
        </div>
      </div>

      {/* Terminal viewport */}
      <div className="flex-1 min-h-0 relative">
        {tabs.map((tab) => (
          <SingleTerminal
            key={`${workspacePath}::${tab.id}::${tab.runKey ?? 0}`}
            tab={tab}
            active={tab.id === activeId}
            theme={theme}
            workspaceKey={workspacePath}
            registerReader={registerReader}
          />
        ))}
      </div>

      {menuAnchor && (
        <LauncherMenu
          anchor={menuAnchor}
          tools={cliTools}
          onLaunch={launchAgent}
          onClose={() => setMenuAnchor(null)}
        />
      )}
    </div>
  );
};
