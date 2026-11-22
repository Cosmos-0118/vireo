import React, { useRef } from 'react';
import type { LiteTheme } from '../themes';
import { usePtyTerminal } from './usePtyTerminal';

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
  const { termRef } = usePtyTerminal(hostRef, {
    sessionKey: `cli::${workspacePath}::${runKey}::${command ?? ''}`,
    cmd: command,
    active,
    theme,
    fontSize: 13.5,
    scrollback: 5000,
    exitMessage: '[Agent session ended]',
    focusOnOpen: true,
  });

  return (
    <div
      className="flex-1 min-h-0 w-full relative overflow-hidden box-border"
      style={{ background: theme.terminal.background, padding: '8px 10px' }}
      onClick={() => termRef.current?.focus()}
    >
      <div ref={hostRef} className="h-full w-full" />
    </div>
  );
};

export default CliAgentPty;
