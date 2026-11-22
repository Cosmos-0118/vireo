import React, { useEffect, useRef, useState } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import '@xterm/xterm/css/xterm.css';
import { Plus, Send, Trash2, X, Terminal as TerminalIcon } from 'lucide-react';
import type { LiteTheme } from '../themes';
import { wsUrl } from '../api';

export interface TermTabSpec {
  id: string;
  title: string;
  cmd?: string;
}

interface SingleTerminalProps {
  tab: TermTabSpec;
  active: boolean;
  theme: LiteTheme;
  workspaceKey: string;
  onOutputUpdate: (id: string, tail: string) => void;
}

const SingleTerminal: React.FC<SingleTerminalProps> = ({
  tab,
  active,
  theme,
  workspaceKey,
  onOutputUpdate,
}) => {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const bufferRef = useRef<string>('');

  useEffect(() => {
    if (!hostRef.current) return;

    const term = new Terminal({
      cursorBlink: true,
      fontSize: 14,
      lineHeight: 1.38,
      fontFamily: "'JetBrains Mono Variable', 'JetBrains Mono', 'Cascadia Code', Consolas, monospace",
      theme: theme.terminal,
      scrollback: 3000,
    });
    const fit = new FitAddon();
    const links = new WebLinksAddon();
    term.loadAddon(fit);
    term.loadAddon(links);
    term.open(hostRef.current);

    termRef.current = term;
    fitRef.current = fit;

    const fitAndResize = () => {
      try {
        fit.fit();
        const sock = wsRef.current;
        if (sock && sock.readyState === WebSocket.OPEN) {
          sock.send(
            JSON.stringify({
              type: 'resize',
              cols: term.cols,
              rows: term.rows,
            })
          );
        }
      } catch {
        // ignore layout race
      }
    };

    setTimeout(fitAndResize, 40);

    const params = new URLSearchParams({
      cols: String(term.cols || 100),
      rows: String(term.rows || 24),
    });
    if (tab.cmd) params.set('cmd', tab.cmd);

    const sock = new WebSocket(wsUrl(`/ws/pty?${params.toString()}`));
    wsRef.current = sock;

    sock.onopen = () => {
      fitAndResize();
    };

    sock.onmessage = (ev) => {
      const text = typeof ev.data === 'string' ? ev.data : '';
      term.write(text);
      // Keep clean plain-text tail for "Send Output to Agent"
      const stripped = text.replace(/\x1b\[[0-9;]*[a-zA-Z]/g, '');
      bufferRef.current = (bufferRef.current + stripped).slice(-6000);
      onOutputUpdate(tab.id, bufferRef.current);
    };

    sock.onclose = () => {
      term.write('\r\n\x1b[90m[Process exited]\x1b[0m\r\n');
    };

    const dataSub = term.onData((data) => {
      if (sock.readyState === WebSocket.OPEN) {
        sock.send(JSON.stringify({ type: 'input', data }));
      }
    });

    const ro = new ResizeObserver(() => {
      fitAndResize();
    });
    ro.observe(hostRef.current);

    return () => {
      ro.disconnect();
      dataSub.dispose();
      sock.close();
      term.dispose();
    };
    // Re-spawn when tab id or workspace changes
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.id, workspaceKey]);

  useEffect(() => {
    if (termRef.current) {
      termRef.current.options.theme = theme.terminal;
    }
  }, [theme]);

  useEffect(() => {
    if (active && fitRef.current && termRef.current) {
      setTimeout(() => {
        try {
          fitRef.current?.fit();
          termRef.current?.focus();
        } catch {
          // ignore
        }
      }, 30);
    }
  }, [active]);

  return (
    <div
      ref={hostRef}
      className="h-full w-full"
      style={{
        display: active ? 'block' : 'none',
        background: theme.terminal.background,
      }}
    />
  );
};

interface TerminalPanelProps {
  theme: LiteTheme;
  workspacePath: string;
  pendingCommand: { id: number; name: string; command: string } | null;
  onSendOutputToAgent: (output: string) => void;
  onClosePanel?: () => void;
}

export const TerminalPanel: React.FC<TerminalPanelProps> = ({
  theme,
  workspacePath,
  pendingCommand,
  onSendOutputToAgent,
  onClosePanel,
}) => {
  const [tabs, setTabs] = useState<TermTabSpec[]>([
    { id: 'shell-1', title: 'PowerShell' },
  ]);
  const [activeId, setActiveId] = useState<string>('shell-1');
  const tailsRef = useRef<Record<string, string>>({});

  useEffect(() => {
    if (!pendingCommand) return;
    const id = `task-${pendingCommand.id}`;
    setTabs((prev) => [...prev, { id, title: pendingCommand.name, cmd: pendingCommand.command }]);
    setActiveId(id);
  }, [pendingCommand]);

  const addShell = () => {
    const id = `shell-${Date.now()}`;
    setTabs((prev) => [...prev, { id, title: `Shell ${prev.length + 1}` }]);
    setActiveId(id);
  };

  const closeTab = (id: string) => {
    setTabs((prev) => {
      const next = prev.filter((t) => t.id !== id);
      if (next.length === 0) {
        const fresh = { id: `shell-${Date.now()}`, title: 'PowerShell' };
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
    const tail = (tailsRef.current[activeId] || '').trim();
    if (!tail) return;
    const excerpt = tail.slice(-2800);
    onSendOutputToAgent(
      `Here is the latest terminal output. Help diagnose/fix any errors:\n\`\`\`\n${excerpt}\n\`\`\`\n`
    );
  };

  return (
    <div className="flex flex-col h-full bg-[var(--bg-panel)] border-t border-[var(--border)]">
      {/* Terminal tab bar */}
      <div className="flex items-center justify-between px-3 h-9 border-b border-[var(--border)] bg-[var(--bg-sidebar)] shrink-0">
        <div className="flex items-center gap-1.5 overflow-x-auto">
          {tabs.map((t) => (
            <div
              key={t.id}
              onClick={() => setActiveId(t.id)}
              className={`flex items-center gap-1.5 px-3 py-1 text-[12.5px] rounded-md cursor-pointer select-none transition-colors ${
                t.id === activeId
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                  : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
              }`}
            >
              <TerminalIcon size={13} />
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
            className="p-1.5 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] rounded-md cursor-pointer"
            title="New terminal"
          >
            <Plus size={15} />
          </button>
        </div>

        <div className="flex items-center gap-1.5">
          <button
            onClick={sendLastOutput}
            className="px-2.5 py-1 text-xs rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] flex items-center gap-1.5 cursor-pointer transition-colors"
            title="Send recent terminal output to Agent Chat"
          >
            <Send size={12} />
            Send Output to Agent
          </button>
          <button
            onClick={() => closeTab(activeId)}
            className="p-1.5 text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)] rounded-md cursor-pointer"
            title="Kill current terminal"
          >
            <Trash2 size={14} />
          </button>
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
            key={`${workspacePath}::${tab.id}`}
            tab={tab}
            active={tab.id === activeId}
            theme={theme}
            workspaceKey={workspacePath}
            onOutputUpdate={(id, tail) => {
              tailsRef.current[id] = tail;
            }}
          />
        ))}
      </div>
    </div>
  );
};
