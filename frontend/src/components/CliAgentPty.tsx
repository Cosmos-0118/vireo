import React, { useEffect, useRef } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import { wsUrl } from '../api';
import type { LiteTheme } from '../themes';

export interface CliAgentPtyProps {
  command?: string;
  runKey: number;
  theme: LiteTheme;
  workspacePath: string;
}

export const CliAgentPty: React.FC<CliAgentPtyProps> = ({
  command,
  runKey,
  theme,
  workspacePath,
}) => {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);

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

export default CliAgentPty;
