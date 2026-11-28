import { TerminalWindow } from '@phosphor-icons/react';
import { EmptyState } from './ui/EmptyState';
import { withShortcut } from '../keybindings';
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
import { getBootstrap } from '../prefs';

const IS_WINDOWS_UA = typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent);
/** Title of plain shell tabs; hydrated from bootstrap or replaced by the backend's shell name. */
let shellTitle = getBootstrap()?.shell || (IS_WINDOWS_UA ? 'pwsh' : 'Terminal');
const IS_MAC = typeof navigator !== 'undefined' && /Macintosh|Mac OS X/i.test(navigator.userAgent);

export interface TermTabSpec {
  id: string;
  title: string;
  cmd?: string;
  /** 'agent' tabs host a CLI coding agent (claude, codex, ...) and can be restarted. */
  kind?: 'shell' | 'agent';
  /** Bumped to restart the tab's process (new backend session). */
  runKey?: number;
  /** Working directory relative to the workspace root. */
  cwd?: string;
  env?: Record<string, string>;
  /** Set for tabs started from a Run task (one reusable tab per task). */
  taskId?: string;
}

export interface TaskLaunch {
  taskId: string;
  name: string;
  command: string;
  cwd?: string;
  env?: Record<string, string>;
  /** Open a separate tab instead of reusing/restarting the task's tab. */
  newTab?: boolean;
}

export interface PendingTaskCommand {
  id: number;
  action: 'run' | 'stop';
  tasks: TaskLaunch[];
}

/**
 * Every plain shell tab is named after the real shell: "PowerShell", then "PowerShell 2",
 * "PowerShell 3"... using the lowest number not already on screen.
 */
function nextShellTitle(tabs: TermTabSpec[]): string {
  const used = new Set(tabs.map((t) => t.title));
  if (!used.has(shellTitle)) return shellTitle;
  let n = 2;
  while (used.has(`${shellTitle} ${n}`)) n++;
  return `${shellTitle} ${n}`;
}
const ptySid = (workspace: string, tab: TermTabSpec) =>
  `term::${workspace}::${tab.id}::${tab.runKey ?? 0}::${tab.cmd ?? ''}`;

const FALLBACK_AGENT_PRESETS: { id: string; label: string; cmd: string }[] = [
  { id: 'antigravity', label: 'Antigravity', cmd: 'agy' },
  { id: 'claude', label: 'Claude Code', cmd: 'claude' },
  { id: 'codex', label: 'Codex', cmd: 'codex' },
  { id: 'gemini', label: 'Gemini CLI', cmd: 'gemini' },
  { id: 'opencode', label: 'OpenCode', cmd: 'opencode' },
  { id: 'cursor', label: 'Cursor CLI', cmd: 'cursor-agent' },
  { id: 'copilot', label: 'GitHub Copilot', cmd: 'copilot' },
  { id: 'amp', label: 'Amp', cmd: 'amp' },
  { id: 'qwen', label: 'Qwen Code', cmd: 'qwen' },
  { id: 'aider', label: 'Aider', cmd: 'aider' },
  { id: 'cline', label: 'Cline', cmd: 'cline' },
  { id: 'kilo', label: 'Kilo Code', cmd: 'kilo' },
  { id: 'goose', label: 'Goose', cmd: 'goose' },
  { id: 'kiro', label: 'Kiro', cmd: 'kiro-cli' },
];

export interface LiveTask {
  taskId: string;
  title: string;
}
interface SingleTerminalProps {
  tab: TermTabSpec;
  active: boolean;
  visible: boolean;
  theme: LiteTheme;
  workspaceKey: string;
  registerReader: (id: string, reader: (() => string) | null) => void;
  onExit: (sid: string) => void;
}

const SingleTerminal: React.FC<SingleTerminalProps> = ({
  tab,
  active,
  visible,
  theme,
  workspaceKey,
  registerReader,
  onExit,
}) => {
  const hostRef = useRef<HTMLDivElement>(null);
  const isAgent = tab.kind === 'agent';
  const { termRef } = usePtyTerminal(hostRef, {
    sessionKey: ptySid(workspaceKey, tab),
    cmd: tab.cmd,
    cwd: tab.cwd,
    env: tab.env,
    active,
    visible,
    theme,
    // macOS renders type larger than Windows at the same px; 12px matches the UI and VS Code on Mac.
    fontSize: IS_MAC ? 12 : 14,
    scrollback: isAgent ? 3000 : 1500,
    exitMessage: isAgent ? '[Agent session ended]' : '[Process exited]',
    focusOnOpen: isAgent,
    onExit: () => onExit(ptySid(workspaceKey, tab)),
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
        // No right padding: xterm reserves its own scrollbar lane on the right, so padding there
        // would double the gutter and the text would stop well short of the panel edge.
        padding: '6px 0 4px 12px',
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
  const listRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top?: number; bottom?: number; maxHeight?: number }>({
    left: anchor.left,
    bottom: window.innerHeight - anchor.top + 4,
  });

  // Open above the button when it fits, otherwise on whichever side has more
  // room, and cap the height so the menu never runs off-screen.
  useLayoutEffect(() => {
    const el = ref.current;
    const list = listRef.current;
    const w = el?.offsetWidth ?? 320;
    const margin = 8;
    const gap = 4;
    const spaceAbove = anchor.top - gap - margin;
    const spaceBelow = window.innerHeight - anchor.bottom - gap - margin;
    // Natural (uncapped) height: chrome around the list + the list's full content
    const natural = el && list ? el.offsetHeight - list.offsetHeight + list.scrollHeight : 0;
    const above = natural <= spaceAbove || spaceAbove >= spaceBelow;
    const left = Math.max(margin, Math.min(anchor.left, window.innerWidth - w - margin));
    if (above) {
      setPos({ left, bottom: window.innerHeight - anchor.top + gap, maxHeight: Math.max(120, spaceAbove) });
    } else {
      setPos({ left, top: anchor.bottom + gap, maxHeight: Math.max(120, spaceBelow) });
    }
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

  const entries: Array<{
    id: string;
    label: string;
    cmd: string;
    installed?: boolean;
    path?: string | null;
    install_hint?: string;
  }> =
    tools.length > 0
      ? [...tools].sort((a, b) => Number(b.installed) - Number(a.installed))
      : FALLBACK_AGENT_PRESETS;

  return (
    <div
      ref={ref}
      style={{
        position: 'fixed',
        left: pos.left,
        top: pos.top,
        bottom: pos.bottom,
        maxHeight: pos.maxHeight,
        zIndex: 60,
        width: 330,
      }}
      className="flex flex-col rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl py-1 text-[12.5px]"
    >
      <div className="px-3 py-1.5 text-[10.5px] uppercase tracking-wider text-[var(--text-dim)]">
        Launch CLI agent
      </div>
      <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto">
        {entries.map((p) => {
          const installed = p.installed;
          return (
            <div
              key={p.id}
              onClick={() => onLaunch(p.label, p.cmd)}
              className="flex items-center gap-2 px-3 py-1.5 cursor-pointer hover:bg-[var(--bg-hover)]"
              title={
                installed
                  ? p.path || p.cmd
                  : installed === false
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
              <span className="text-[var(--text-primary)] font-medium shrink-0">{p.label}</span>
              <span className="font-mono text-[11px] text-[var(--text-dim)] truncate min-w-0 flex-1">
                {installed ? p.path || '' : ''}
              </span>
              {installed === false && p.install_hint && (
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    copyHint(p.id, p.install_hint!);
                  }}
                  className="flex items-center gap-1 px-1.5 py-0.5 text-[10.5px] rounded border border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-active)] cursor-pointer shrink-0"
                  title={p.install_hint}
                >
                  {copied === p.id ? <Check size={10} /> : <Copy size={10} />}
                  {copied === p.id ? 'Copied' : 'Install cmd'}
                </button>
              )}
            </div>
          );
        })}
      </div>
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
  pendingCommand: PendingTaskCommand | null;
  /** Reports which tasks currently own a terminal tab (task ids). */
  onTaskTabsChange?: (taskIds: string[]) => void;
  /** Increment to open the "Launch CLI agent" menu (e.g. from the Agent Chat header). */
  launcherSignal?: number;
  /** Increment to open a new shell tab (the initial value is ignored). */
  newShellSignal?: number;
  /** Workspace-relative working directory for the shell opened by `newShellSignal`. */
  newShellCwd?: string;
  maximized?: boolean;
  onToggleMaximize?: () => void;
  onSendOutputToAgent: (output: string) => void;
  onClosePanel?: () => void;
  /** False while the dock is collapsed: tabs stay alive but drop GPU/blink work. */
  visible?: boolean;
  /** Task tabs whose process is still running (for the status bar). */
  onLiveTasksChange?: (tasks: LiveTask[]) => void;
  /** Bring the given task's tab to the front; `n` changes on every request. */
  focusTask?: { taskId: string; n: number } | null;
}

export const TerminalPanel: React.FC<TerminalPanelProps> = ({
  theme,
  workspacePath,
  pendingCommand,
  onTaskTabsChange,
  launcherSignal = 0,
  newShellSignal = 0,
  newShellCwd = '',
  maximized = false,
  onToggleMaximize,
  onSendOutputToAgent,
  onClosePanel,
  visible = true,
  onLiveTasksChange,
  focusTask,
}) => {
  // Backend sessions whose process has ended (keyed by session id, so a restart is live again).
  const [exited, setExited] = useState<ReadonlySet<string>>(() => new Set());
  const markExited = React.useCallback(
    (sid: string) => setExited((s) => (s.has(sid) ? s : new Set(s).add(sid))),
    []
  );
  const [tabs, setTabs] = useState<TermTabSpec[]>([
    { id: 'shell-1', title: shellTitle, kind: 'shell' },
  ]);
  const [activeId, setActiveId] = useState<string>('shell-1');
  // Name plain shell tabs after the real shell (zsh/bash on macOS/Linux) once the backend says.
  useEffect(() => {
    if (getBootstrap()?.shell || IS_WINDOWS_UA) return;
    let cancelled = false;
    api
      .shellInfo()
      .then(({ name }) => {
        if (cancelled || !name || name === shellTitle) return;
        const prev = shellTitle;
        shellTitle = name;
        // "zsh", "zsh 2", ... keep their numbers.
        setTabs((ts) =>
          ts.map((t) =>
            t.kind === 'shell' && !t.taskId && (t.title === prev || t.title.startsWith(`${prev} `))
              ? { ...t, title: name + t.title.slice(prev.length) }
              : t
          )
        );
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);
  const [engine, setEngine] = useTerminalEngine();
  const [cliTools, setCliTools] = useState<CliToolStatus[]>([]);
  const [menuAnchor, setMenuAnchor] = useState<DOMRect | null>(null);
  const launcherBtnRef = useRef<HTMLButtonElement>(null);
  const readersRef = useRef<Record<string, () => string>>({});
  const registerReader = React.useCallback((id: string, reader: (() => string) | null) => {
    if (reader) readersRef.current[id] = reader;
    else delete readersRef.current[id];
  }, []);

  // Run tasks reuse one tab per task id: re-running restarts the process in the same tab
  // instead of piling up dead tabs. `newTab` opts out; `stop` closes the task's tab.
  const handledCmdRef = useRef(0);
  useEffect(() => {
    if (!pendingCommand || handledCmdRef.current === pendingCommand.id) return;
    handledCmdRef.current = pendingCommand.id;

    if (pendingCommand.action === 'stop') {
      for (const task of pendingCommand.tasks) {
        for (const t of tabs.filter((x) => x.taskId === task.taskId)) closeTab(t.id);
      }
      return;
    }

    let next = tabs;
    let lastId = '';
    pendingCommand.tasks.forEach((task, i) => {
      const id = task.newTab
        ? `task::${task.taskId}#${pendingCommand.id}-${i}`
        : `task::${task.taskId}`;
      lastId = id;
      const spec: Partial<TermTabSpec> = {
        title: task.name,
        cmd: task.command,
        cwd: task.cwd || undefined,
        env: task.env && Object.keys(task.env).length > 0 ? task.env : undefined,
      };
      const existing = next.find((t) => t.id === id);
      if (existing) {
        killPtySession(ptySid(workspacePath, existing));
        next = next.map((t) =>
          t.id === id ? { ...t, ...spec, runKey: (t.runKey ?? 0) + 1 } : t
        );
      } else {
        next = [...next, { id, kind: 'shell', taskId: task.taskId, title: task.name, ...spec }];
      }
    });
    setTabs(next);
    if (lastId) setActiveId(lastId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingCommand]);

  useEffect(() => {
    onTaskTabsChange?.(tabs.flatMap((t) => (t.taskId ? [t.taskId] : [])));
  }, [tabs, onTaskTabsChange]);

  useEffect(() => {
    onLiveTasksChange?.(
      tabs.flatMap((t) =>
        t.taskId && !exited.has(ptySid(workspacePath, t)) ? [{ taskId: t.taskId, title: t.title }] : []
      )
    );
  }, [tabs, exited, workspacePath, onLiveTasksChange]);

  useEffect(() => {
    if (!focusTask) return;
    const t = tabs.find((x) => x.taskId === focusTask.taskId);
    if (t) setActiveId(t.id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusTask]);

  const openMenu = () => {
    // The button can be unmounted or display:none (dock just revealed); fall back to the
    // bottom-left corner instead of silently refusing to open the menu.
    const measured = launcherBtnRef.current?.getBoundingClientRect();
    const rect =
      measured && (measured.width > 0 || measured.height > 0)
        ? measured
        : new DOMRect(12, window.innerHeight - 40, 0, 0);
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

  const addShell = (cwd?: string) => {
    const id = `shell-${Date.now()}`;
    setTabs((prev) => [...prev, { id, title: nextShellTitle(prev), kind: 'shell', ...(cwd ? { cwd } : {}) }]);
    setActiveId(id);
  };

  const handledShellRef = useRef(newShellSignal);
  useEffect(() => {
    if (newShellSignal === handledShellRef.current) return;
    handledShellRef.current = newShellSignal;
    addShell(newShellCwd || undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [newShellSignal]);

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
        const fresh: TermTabSpec = { id: `shell-${Date.now()}`, title: shellTitle, kind: 'shell' };
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
    <div className="flex flex-col h-full bg-[var(--bg-panel)] border-t border-[var(--divider)]">
      {/* Terminal tab bar */}
      <div
        style={{ background: theme.terminal.background }}
        className="@container relative flex items-stretch justify-between gap-2 pl-1.5 pr-2 h-9 shrink-0"
      >
        {/* One unbroken hairline separates the header from the terminal (same background on both
            sides); the active tab's underline sits on top of it. */}
        <div className="pointer-events-none absolute inset-x-0 bottom-0 h-px bg-[var(--divider)]" />
        <div className="flex items-stretch gap-0.5 min-w-0 flex-1 overflow-x-auto overflow-y-hidden [scrollbar-width:none]">
          {tabs.map((t) => (
            <div
              key={t.id}
              onClick={() => setActiveId(t.id)}
              className={`group relative flex items-center gap-1.5 min-w-0 h-full whitespace-nowrap pl-2.5 pr-1.5 text-[12.5px] cursor-pointer select-none transition-colors ${
                t.id === activeId
                  ? 'text-[var(--text-primary)]'
                  : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
              }`}
            >
              {t.id === activeId && (
                <span className="pointer-events-none absolute inset-x-1.5 bottom-0 h-0.5 rounded-full bg-[var(--accent)]" />
              )}
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
                  className={`p-0.5 rounded text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-opacity ${
                    t.id === activeId ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 focus-visible:opacity-100'
                  }`}
                  title="Close terminal"
                >
                  <X size={12} />
                </button>
              )}
            </div>
          ))}
          <button
            onClick={() => addShell()}
            className="shrink-0 self-center ml-1 p-1.5 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] rounded-md cursor-pointer"
            title="New terminal"
          >
            <Plus size={15} />
          </button>
          <button
            ref={launcherBtnRef}
            data-launcher-btn
            onClick={() => (menuAnchor ? setMenuAnchor(null) : openMenu())}
            className={`shrink-0 self-center flex items-center gap-1 px-2 py-1 text-[12px] rounded-md cursor-pointer whitespace-nowrap transition-colors ${
              menuAnchor
                ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
            }`}
            title="Launch a CLI agent (Antigravity, Claude Code, Codex, Gemini, OpenCode, Cursor, Copilot…)"
          >
            <Sparkles size={13} />
            <span className="hidden @[520px]:inline">Launch Agent</span>
            <ChevronDown size={12} />
          </button>
        </div>

        <div className="flex items-center gap-1.5 shrink-0 self-center">
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
              title={withShortcut('Hide terminal', 'toggleTerminal')}
            >
              <X size={15} />
            </button>
          )}
        </div>
      </div>

      {/* Terminal viewport */}
      <div className="flex-1 min-h-0 relative" style={{ background: theme.terminal.background }}>
        {!workspacePath && (
          <EmptyState
            size="sm"
            icon={TerminalWindow}
            title="No folder open"
            description="Open a folder to start a terminal session."
            className="absolute inset-0"
          />
        )}
        {workspacePath && tabs.map((tab) => (
          <SingleTerminal
            key={`${workspacePath}::${tab.id}::${tab.runKey ?? 0}`}
            tab={tab}
            active={tab.id === activeId}
            visible={visible}
            theme={theme}
            workspaceKey={workspacePath}
            registerReader={registerReader}
            onExit={markExited}
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
