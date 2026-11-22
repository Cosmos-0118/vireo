import React, { useEffect, useRef } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import '@xterm/xterm/css/xterm.css';
import { wsUrl } from '../api';
import type { LiteTheme } from '../themes';

export interface CliAgentPtyProps {
  command?: string;
  runKey: number;
  active?: boolean;
  theme: LiteTheme;
  workspacePath: string;
}

export const CliAgentPty: React.FC<CliAgentPtyProps> = ({
  command,
  runKey,
  active = true,
  theme,
  workspacePath,
}) => {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const sockRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    if (!hostRef.current) return;
    let disposed = false;

    const term = new Terminal({
      cursorBlink: true,
      fontSize: 13.5,
      lineHeight: 1.38,
      fontFamily: "'JetBrains Mono Variable', 'JetBrains Mono', 'Cascadia Code', Consolas, monospace",
      theme: theme.terminal,
      scrollback: 5000,
    });
    const fit = new FitAddon();
    const links = new WebLinksAddon();
    term.loadAddon(fit);
    term.loadAddon(links);
    term.open(hostRef.current);

    termRef.current = term;
    fitRef.current = fit;

    // Fit before connecting so TUI CLI agents receive the actual panel width/height
    try {
      fit.fit();
    } catch {
      // ignore initial layout race
    }

    const initCols = Math.max(20, term.cols || 60);
    const initRows = Math.max(5, term.rows || 24);
    const params = new URLSearchParams({
      cols: String(initCols),
      rows: String(initRows),
    });
    if (command) params.set('cmd', command);

    const sock = new WebSocket(wsUrl(`/ws/pty?${params.toString()}`));
    sockRef.current = sock;

    const resize = () => {
      if (disposed) return;
      try {
        fit.fit();
        if (sock.readyState === WebSocket.OPEN) {
          sock.send(JSON.stringify({ type: 'resize', cols: term.cols, rows: term.rows }));
        }
      } catch {
        // ignore layout race
      }
    };

    // Standard terminal copy/paste: Ctrl+C copies when text is selected, Ctrl+V pastes clipboard
    term.attachCustomKeyEventHandler((ev) => {
      if (ev.type !== 'keydown') return true;
      const mod = ev.ctrlKey || ev.metaKey;
      if (mod && !ev.altKey && ev.key.toLowerCase() === 'c' && term.hasSelection()) {
        const sel = term.getSelection();
        if (sel) {
          navigator.clipboard?.writeText(sel).catch(() => {});
          term.clearSelection();
        }
        return false;
      }
      if (mod && !ev.altKey && ev.key.toLowerCase() === 'v') {
        ev.preventDefault();
        navigator.clipboard
          ?.readText()
          .then((text) => {
            if (!disposed && text && sock.readyState === WebSocket.OPEN) {
              sock.send(JSON.stringify({ type: 'input', data: text }));
            }
          })
          .catch(() => {});
        return false;
      }
      return true;
    });

    sock.onopen = () => {
      if (disposed) return;
      resize();
      term.focus();
    };
    sock.onmessage = (e) => {
      if (!disposed && typeof e.data === 'string') {
        term.write(e.data);
      }
    };
    sock.onerror = () => {
      if (!disposed) {
        term.write('\r\n\x1b[31m[CLI Agent WebSocket connection error]\x1b[0m\r\n');
      }
    };
    sock.onclose = () => {
      if (!disposed) {
        term.write('\r\n\x1b[90m[Agent session ended]\x1b[0m\r\n');
      }
    };

    const sub = term.onData((data) => {
      if (!disposed && sock.readyState === WebSocket.OPEN) {
        sock.send(JSON.stringify({ type: 'input', data }));
      }
    });

    const ro = new ResizeObserver(resize);
    ro.observe(hostRef.current);
    const timer = window.setTimeout(() => {
      resize();
      if (!disposed) term.focus();
    }, 60);

    return () => {
      disposed = true;
      window.clearTimeout(timer);
      ro.disconnect();
      sub.dispose();
      sock.close();
      sockRef.current = null;
      fitRef.current = null;
      termRef.current = null;
      term.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [command, runKey, workspacePath]);

  useEffect(() => {
    if (termRef.current) {
      termRef.current.options.theme = theme.terminal;
    }
  }, [theme]);

  useEffect(() => {
    if (!active) return;
    const t = window.setTimeout(() => {
      try {
        fitRef.current?.fit();
        const sock = sockRef.current;
        const term = termRef.current;
        if (sock && term && sock.readyState === WebSocket.OPEN) {
          sock.send(JSON.stringify({ type: 'resize', cols: term.cols, rows: term.rows }));
        }
        term?.focus();
      } catch {
        // ignore
      }
    }, 30);
    return () => window.clearTimeout(t);
  }, [active]);

  return (
    <div
      className="flex-1 min-h-0 w-full relative overflow-hidden"
      style={{ background: theme.terminal.background }}
      onClick={() => termRef.current?.focus()}
    >
      <div ref={hostRef} className="h-full w-full" />
    </div>
  );
};

export default CliAgentPty;
