import React, { useCallback, useEffect, useRef, useState } from 'react';
import {
  Bot,
  FolderGit2,
  GitBranch,
  GitCompare,
  Palette,
  PanelBottom,
  PanelLeft,
  PanelRight,
  Play,
  Radio,
  Search,
  Upload,
  X,
  FileCode,
} from 'lucide-react';
import { api, wsUrl } from './api';
import {
  applyTheme,
  BUILTIN_THEMES,
  LiteTheme,
  parseVsCodeThemeJson,
} from './themes';
import type {
  GitDiffRes,
  GitStatusRes,
  OpenTab,
  TaskItem,
  WorkspaceInfo,
} from './types';
import { Sidebar, SidebarTab } from './components/Sidebar';
import { DiffViewer } from './components/DiffViewer';
import { CodeEditor } from './components/CodeEditor';
import { TerminalPanel } from './components/TerminalPanel';
import { AgentPanel } from './components/AgentPanel';
import { QuickOpen } from './components/QuickOpen';

export function App() {
  // Themes (built-in + custom imported VS Code themes)
  const [themes, setThemes] = useState<LiteTheme[]>(() => {
    try {
      const extra = localStorage.getItem('vireo-custom-themes');
      if (extra) return [...BUILTIN_THEMES, ...JSON.parse(extra)];
    } catch {
      // ignore
    }
    return BUILTIN_THEMES;
  });
  const [themeId, setThemeId] = useState<string>(
    () => localStorage.getItem('vireo-theme') || BUILTIN_THEMES[0].id
  );
  const activeTheme = themes.find((t) => t.id === themeId) || BUILTIN_THEMES[0];

  useEffect(() => {
    applyTheme(activeTheme);
    localStorage.setItem('vireo-theme', activeTheme.id);
  }, [activeTheme]);

  // Workspace & Git state
  const [workspace, setWorkspace] = useState<WorkspaceInfo>({ path: '', name: 'Vireo' });
  const [editingWs, setEditingWs] = useState(false);
  const [wsDraft, setWsDraft] = useState('');
  const [gitStatus, setGitStatus] = useState<GitStatusRes | null>(null);
  const [allFiles, setAllFiles] = useState<string[]>([]);
  const [tasks, setTasks] = useState<TaskItem[]>([]);
  const [liveConnected, setLiveConnected] = useState(false);

  // Layout toggles & sizes
  const [showSidebar, setShowSidebar] = useState(true);
  const [showAgent, setShowAgent] = useState(true);
  const [showTerminal, setShowTerminal] = useState(true);
  const [sidebarTab, setSidebarTab] = useState<SidebarTab>('changes');
  const [sidebarWidth, setSidebarWidth] = useState(290);
  const [agentWidth, setAgentWidth] = useState(370);
  const [terminalHeight, setTerminalHeight] = useState(230);

  // Menus & modals
  const [showThemeMenu, setShowThemeMenu] = useState(false);
  const [showRunMenu, setShowRunMenu] = useState(false);
  const [quickOpenVisible, setQuickOpenVisible] = useState(false);
  const [toast, setToast] = useState<{ msg: string; isError?: boolean } | null>(null);

  // Tabs & Active content
  const [tabs, setTabs] = useState<OpenTab[]>([]);
  const [activeTabPath, setActiveTabPath] = useState<string | null>(null);
  const [diffData, setDiffData] = useState<GitDiffRes | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [fileData, setFileData] = useState<{ content: string; binary: boolean } | null>(null);

  // Cross-panel triggers
  const [pendingCmd, setPendingCmd] = useState<{
    id: number;
    name: string;
    command: string;
  } | null>(null);
  const [agentDraftAppend, setAgentDraftAppend] = useState<{
    id: number;
    text: string;
  } | null>(null);

  const themeFileInputRef = useRef<HTMLInputElement>(null);

  const showToast = useCallback((msg: string, isError = false) => {
    setToast({ msg, isError });
    setTimeout(() => setToast((cur) => (cur?.msg === msg ? null : cur)), 3600);
  }, []);

  const refreshGit = useCallback(async () => {
    try {
      const status = await api.gitStatus();
      setGitStatus(status);
    } catch {
      // ignore transient error
    }
  }, []);

  const loadWorkspaceData = useCallback(async () => {
    try {
      const [ws, status, files, taskList] = await Promise.all([
        api.getWorkspace(),
        api.gitStatus(),
        api.listFiles(),
        api.getTasks(),
      ]);
      setWorkspace(ws);
      setWsDraft(ws.path);
      setGitStatus(status);
      setAllFiles(files);
      setTasks(taskList);

      // Auto-open the first modified file's diff if nothing is open yet
      setTabs((prev) => {
        if (prev.length === 0 && status.files.length > 0) {
          const first = status.files[0];
          setActiveTabPath(first.path);
          return [{ path: first.path, mode: 'diff', staged: first.staged && !first.unstaged }];
        }
        return prev;
      });
    } catch (e: unknown) {
      showToast(e instanceof Error ? e.message : 'Failed to load workspace', true);
    }
  }, [showToast]);

  useEffect(() => {
    loadWorkspaceData();
  }, [loadWorkspaceData]);

  // Active tab loader
  const activeTab = tabs.find((t) => t.path === activeTabPath) || null;

  const reloadActiveContent = useCallback(async () => {
    if (!activeTab) {
      setDiffData(null);
      setFileData(null);
      return;
    }
    if (activeTab.mode === 'diff') {
      setDiffLoading(true);
      try {
        const d = await api.gitDiff(activeTab.path, !!activeTab.staged);
        setDiffData(d);
      } catch {
        setDiffData(null);
      } finally {
        setDiffLoading(false);
      }
    } else {
      try {
        const f = await api.readFile(activeTab.path);
        setFileData({ content: f.content, binary: f.binary });
      } catch {
        setFileData(null);
      }
    }
  }, [activeTab]);

  useEffect(() => {
    reloadActiveContent();
  }, [reloadActiveContent]);

  // Live file-watcher WebSocket so diffs auto-refresh as agents code!
  useEffect(() => {
    let closed = false;
    let sock: WebSocket | null = null;
    let timer: number | undefined;

    const connect = () => {
      if (closed) return;
      sock = new WebSocket(wsUrl('/ws/events'));
      sock.onopen = () => setLiveConnected(true);
      sock.onclose = () => {
        setLiveConnected(false);
        if (!closed) {
          timer = window.setTimeout(connect, 2000);
        }
      };
      sock.onmessage = (ev) => {
        try {
          const data = JSON.parse(ev.data);
          if (data.type === 'fs_changed') {
            refreshGit();
            reloadActiveContent();
          } else if (data.type === 'workspace_switched') {
            setTabs([]);
            setActiveTabPath(null);
            loadWorkspaceData();
          } else if (data.type === 'checkpoint_created') {
            refreshGit();
          }
        } catch {
          // ignore
        }
      };
    };

    connect();
    return () => {
      closed = true;
      if (timer) clearTimeout(timer);
      sock?.close();
    };
  }, [refreshGit, reloadActiveContent, loadWorkspaceData]);

  // Global VS Code keyboard shortcuts
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'p') {
        e.preventDefault();
        setQuickOpenVisible((v) => !v);
      } else if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 'f') {
        e.preventDefault();
        setShowSidebar(true);
        setSidebarTab('search');
      } else if ((e.ctrlKey || e.metaKey) && e.key === '`') {
        e.preventDefault();
        setShowTerminal((v) => !v);
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'b') {
        e.preventDefault();
        setShowSidebar((v) => !v);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const openDiff = useCallback((path: string, staged = false) => {
    setTabs((prev) => {
      const existing = prev.find((t) => t.path === path);
      if (existing) {
        return prev.map((t) => (t.path === path ? { ...t, mode: 'diff', staged } : t));
      }
      return [...prev, { path, mode: 'diff', staged }];
    });
    setActiveTabPath(path);
  }, []);

  const openFile = useCallback((path: string, line?: number) => {
    setTabs((prev) => {
      const existing = prev.find((t) => t.path === path);
      if (existing) {
        return prev.map((t) => (t.path === path ? { ...t, mode: 'code', line } : t));
      }
      return [...prev, { path, mode: 'code', line }];
    });
    setActiveTabPath(path);
  }, []);

  const closeTab = (path: string) => {
    setTabs((prev) => {
      const next = prev.filter((t) => t.path !== path);
      if (activeTabPath === path) {
        setActiveTabPath(next.length > 0 ? next[next.length - 1].path : null);
      }
      return next;
    });
  };

  const handleRunTask = (task: TaskItem) => {
    setShowRunMenu(false);
    setShowTerminal(true);
    setPendingCmd({ id: Date.now(), name: task.name, command: task.command });
  };

  const handleAskAgent = (text: string) => {
    setShowAgent(true);
    setAgentDraftAppend({ id: Date.now(), text });
  };

  const handleSwitchWorkspace = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!wsDraft.trim()) return;
    try {
      const info = await api.setWorkspace(wsDraft.trim());
      setWorkspace(info);
      setEditingWs(false);
      setTabs([]);
      setActiveTabPath(null);
      await loadWorkspaceData();
      showToast(`Opened workspace: ${info.name}`);
    } catch (err: unknown) {
      showToast(err instanceof Error ? err.message : 'Failed to open path', true);
    }
  };

  const handleImportVsCodeTheme = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    try {
      const text = await file.text();
      const parsed = parseVsCodeThemeJson(text, file.name.replace(/\.json$/i, ''));
      setThemes((prev) => {
        const next = [...prev.filter((t) => t.id !== parsed.id), parsed];
        const customOnly = next.filter(
          (t) => !BUILTIN_THEMES.some((b) => b.id === t.id)
        );
        localStorage.setItem('vireo-custom-themes', JSON.stringify(customOnly));
        return next;
      });
      setThemeId(parsed.id);
      setShowThemeMenu(false);
      showToast(`Imported theme: ${parsed.name}`);
    } catch {
      showToast('Invalid VS Code theme JSON file', true);
    }
    e.target.value = '';
  };

  // Drag-resize helper
  const startDrag = (
    axis: 'x' | 'y',
    onDelta: (delta: number) => void
  ) => (e: React.MouseEvent) => {
    e.preventDefault();
    let last = axis === 'x' ? e.clientX : e.clientY;
    const onMove = (ev: MouseEvent) => {
      const cur = axis === 'x' ? ev.clientX : ev.clientY;
      onDelta(cur - last);
      last = cur;
    };
    const onUp = () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  };

  const activeFileStatus = activeTab
    ? gitStatus?.files.find((f) => f.path === activeTab.path)
    : undefined;

  return (
    <div className="h-screen w-screen flex flex-col overflow-hidden bg-[var(--bg-base)] text-[var(--text-primary)]">
      {/* Top Workbench Header */}
      <header className="h-10 px-2.5 flex items-center justify-between border-b border-[var(--border)] bg-[var(--bg-activity)] shrink-0 gap-2 select-none">
        {/* Left: Brand & Workspace Path */}
        <div className="flex items-center gap-2 min-w-0">
          <div className="flex items-center gap-1.5 font-semibold text-xs text-[var(--text-primary)] shrink-0">
            <span className="w-5 h-5 rounded bg-[var(--accent)] text-[var(--accent-fg)] flex items-center justify-center font-bold text-[11px]">
              V
            </span>
            <span>Vireo</span>
          </div>

          <span className="text-[var(--text-dim)]">/</span>

          {editingWs ? (
            <form onSubmit={handleSwitchWorkspace} className="flex items-center gap-1">
              <input
                value={wsDraft}
                onChange={(e) => setWsDraft(e.target.value)}
                onBlur={() => setEditingWs(false)}
                autoFocus
                className="w-72 px-2 py-0.5 text-xs font-mono rounded bg-[var(--bg-input)] border border-[var(--accent)] text-[var(--text-primary)] outline-none"
                placeholder="C:\path\to\repo"
              />
            </form>
          ) : (
            <button
              onClick={() => {
                setWsDraft(workspace.path);
                setEditingWs(true);
              }}
              className="flex items-center gap-1.5 px-2 py-1 rounded hover:bg-[var(--bg-hover)] text-xs text-[var(--text-primary)] max-w-[260px] cursor-pointer"
              title={`Workspace: ${workspace.path} (click to switch folder)`}
            >
              <FolderGit2 size={13} className="text-[var(--accent)] shrink-0" />
              <span className="truncate font-medium">{workspace.name}</span>
            </button>
          )}
        </div>

        {/* Center: Quick Open Pill */}
        <button
          onClick={() => setQuickOpenVisible(true)}
          className="hidden md:flex items-center gap-2 px-3 py-1 w-80 rounded border border-[var(--border)] bg-[var(--bg-input)] hover:border-[var(--accent)] text-xs text-[var(--text-dim)] cursor-pointer"
        >
          <Search size={12} />
          <span className="flex-1 text-left truncate">
            Quick Open files, themes, tasks...
          </span>
          <kbd className="text-[10px] px-1 rounded bg-[var(--bg-elevated)] text-[var(--text-muted)]">
            Ctrl+P
          </kbd>
        </button>

        {/* Right: Run Tasks, Themes, Pane Toggles */}
        <div className="flex items-center gap-1.5 shrink-0">
          {/* Run Scripts Dropdown */}
          <div className="relative">
            <button
              onClick={() => {
                setShowRunMenu((v) => !v);
                setShowThemeMenu(false);
              }}
              className="px-2 py-1 text-xs rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] flex items-center gap-1 cursor-pointer"
              title="Run detected project scripts"
            >
              <Play size={12} className="text-[var(--added-fg)]" />
              Run
            </button>
            {showRunMenu && (
              <div className="absolute right-0 mt-1 w-64 rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] shadow-xl z-40 py-1">
                <div className="px-2.5 py-1 text-[10px] uppercase font-semibold text-[var(--text-dim)]">
                  Detected Project Tasks
                </div>
                {tasks.length === 0 ? (
                  <div className="px-3 py-2 text-xs text-[var(--text-dim)]">
                    No package.json / Cargo.toml tasks found.
                  </div>
                ) : (
                  tasks.map((t) => (
                    <button
                      key={`${t.source}-${t.name}`}
                      onClick={() => handleRunTask(t)}
                      className="w-full text-left px-3 py-1.5 hover:bg-[var(--bg-hover)] flex flex-col cursor-pointer"
                    >
                      <span className="text-xs text-[var(--text-primary)]">{t.name}</span>
                      <span className="text-[10px] font-mono text-[var(--text-dim)] truncate">
                        {t.command}
                      </span>
                    </button>
                  ))
                )}
              </div>
            )}
          </div>

          {/* Theme Picker Dropdown */}
          <div className="relative">
            <button
              onClick={() => {
                setShowThemeMenu((v) => !v);
                setShowRunMenu(false);
              }}
              className="px-2 py-1 text-xs rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] flex items-center gap-1 cursor-pointer"
              title="Switch theme or import a VS Code JSON theme"
            >
              <Palette size={12} className="text-[var(--accent)]" />
              <span className="hidden sm:inline">{activeTheme.name}</span>
            </button>
            {showThemeMenu && (
              <div className="absolute right-0 mt-1 w-56 rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] shadow-xl z-40 py-1">
                <div className="px-2.5 py-1 text-[10px] uppercase font-semibold text-[var(--text-dim)]">
                  Color Themes
                </div>
                {themes.map((th) => (
                  <button
                    key={th.id}
                    onClick={() => {
                      setThemeId(th.id);
                      setShowThemeMenu(false);
                    }}
                    className={`w-full text-left px-3 py-1.5 text-xs flex items-center justify-between cursor-pointer ${
                      th.id === activeTheme.id
                        ? 'bg-[var(--accent)] text-[var(--accent-fg)]'
                        : 'hover:bg-[var(--bg-hover)] text-[var(--text-primary)]'
                    }`}
                  >
                    <span>{th.name}</span>
                    <span
                      className="w-3 h-3 rounded-full border border-white/20"
                      style={{ background: th.vars['--bg-base'] }}
                    />
                  </button>
                ))}
                <div className="border-t border-[var(--border)] mt-1 pt-1 px-2">
                  <button
                    onClick={() => themeFileInputRef.current?.click()}
                    className="w-full py-1.5 px-2 text-xs rounded hover:bg-[var(--bg-hover)] text-[var(--accent)] flex items-center gap-1.5 cursor-pointer"
                  >
                    <Upload size={12} />
                    Import VS Code Theme (.json)
                  </button>
                  <input
                    ref={themeFileInputRef}
                    type="file"
                    accept=".json"
                    onChange={handleImportVsCodeTheme}
                    className="hidden"
                  />
                </div>
              </div>
            )}
          </div>

          <div className="h-4 w-[1px] bg-[var(--border)] mx-0.5" />

          {/* Layout Toggles */}
          <button
            onClick={() => setShowSidebar((v) => !v)}
            className={`p-1.5 rounded cursor-pointer ${
              showSidebar
                ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
            }`}
            title="Toggle Sidebar (Ctrl+B)"
          >
            <PanelLeft size={14} />
          </button>
          <button
            onClick={() => setShowTerminal((v) => !v)}
            className={`p-1.5 rounded cursor-pointer ${
              showTerminal
                ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
            }`}
            title="Toggle Terminal (Ctrl+`)"
          >
            <PanelBottom size={14} />
          </button>
          <button
            onClick={() => setShowAgent((v) => !v)}
            className={`p-1.5 rounded cursor-pointer ${
              showAgent
                ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
            }`}
            title="Toggle Agent Panel"
          >
            <PanelRight size={14} />
          </button>
        </div>
      </header>

      {/* Main Workbench Area */}
      <div className="flex-1 flex min-h-0 overflow-hidden">
        {/* Left Sidebar */}
        {showSidebar && (
          <>
            <div style={{ width: sidebarWidth }} className="shrink-0 h-full min-w-[210px]">
              <Sidebar
                activeTab={sidebarTab}
                onTabChange={setSidebarTab}
                gitStatus={gitStatus}
                selectedPath={activeTabPath}
                workspacePath={workspace.path}
                onOpenDiff={openDiff}
                onOpenFile={openFile}
                onRefreshGit={refreshGit}
                onToast={showToast}
              />
            </div>
            <div
              onMouseDown={startDrag('x', (dx) =>
                setSidebarWidth((w) => Math.max(210, Math.min(500, w + dx)))
              )}
              className="w-1 cursor-col-resize hover:bg-[var(--accent)]/60 shrink-0"
            />
          </>
        )}

        {/* Center Column: Diff/Code + Bottom Terminal */}
        <div className="flex-1 flex flex-col min-w-0 min-h-0">
          {/* Open Tabs Bar */}
          {tabs.length > 0 && (
            <div className="flex items-center h-8 bg-[var(--bg-sidebar)] border-b border-[var(--border)] overflow-x-auto shrink-0">
              {tabs.map((tab) => {
                const active = tab.path === activeTabPath;
                const name = tab.path.split('/').pop() || tab.path;
                return (
                  <div
                    key={tab.path}
                    onClick={() => setActiveTabPath(tab.path)}
                    className={`flex items-center gap-1.5 px-3 h-full text-xs border-r border-[var(--border)] cursor-pointer select-none shrink-0 ${
                      active
                        ? 'bg-[var(--bg-base)] text-[var(--text-primary)] border-t-2 border-t-[var(--accent)]'
                        : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                    }`}
                  >
                    {tab.mode === 'diff' ? (
                      <GitCompare size={12} className="text-[var(--modified-fg)]" />
                    ) : (
                      <FileCode size={12} className="text-[var(--text-muted)]" />
                    )}
                    <span>{name}</span>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        closeTab(tab.path);
                      }}
                      className="ml-1 hover:text-[var(--danger)] cursor-pointer"
                    >
                      <X size={11} />
                    </button>
                  </div>
                );
              })}
            </div>
          )}

          {/* Main Viewer Stage */}
          <div className="flex-1 flex flex-col min-h-0">
            {!activeTab ? (
              <div className="flex-1 flex flex-col items-center justify-center p-8 text-center gap-4">
                <div className="w-12 h-12 rounded-xl bg-[var(--bg-elevated)] border border-[var(--border)] flex items-center justify-center">
                  <Bot size={24} className="text-[var(--accent)]" />
                </div>
                <div className="space-y-1 max-w-md">
                  <h2 className="text-base font-semibold text-[var(--text-primary)]">
                    Vireo — Agent Review Workbench
                  </h2>
                  <p className="text-xs text-[var(--text-muted)] leading-relaxed">
                    Ultra-lightweight workspace for reviewing live agent diffs, running commands,
                    and shipping commits to GitHub without VS Code overhead.
                  </p>
                </div>
                <div className="flex flex-wrap items-center justify-center gap-2 text-xs">
                  <button
                    onClick={() => setQuickOpenVisible(true)}
                    className="px-3 py-1.5 rounded bg-[var(--accent)] text-[var(--accent-fg)] cursor-pointer"
                  >
                    Quick Open (Ctrl+P)
                  </button>
                  <button
                    onClick={() => {
                      setShowSidebar(true);
                      setSidebarTab('explorer');
                    }}
                    className="px-3 py-1.5 rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] cursor-pointer"
                  >
                    Browse Files
                  </button>
                  <button
                    onClick={() => setShowTerminal(true)}
                    className="px-3 py-1.5 rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] cursor-pointer"
                  >
                    Open Terminal (Ctrl+`)
                  </button>
                </div>
              </div>
            ) : activeTab.mode === 'diff' ? (
              <DiffViewer
                diff={diffData}
                fileStatus={activeFileStatus}
                loading={diffLoading}
                onStage={async () => {
                  await api.gitStage({ paths: [activeTab.path] });
                  await refreshGit();
                  reloadActiveContent();
                }}
                onUnstage={async () => {
                  await api.gitUnstage({ paths: [activeTab.path] });
                  await refreshGit();
                  reloadActiveContent();
                }}
                onDiscard={async () => {
                  await api.gitDiscard(activeTab.path, !!activeFileStatus?.untracked);
                  await refreshGit();
                  reloadActiveContent();
                  showToast(`Discarded changes in ${activeTab.path}`);
                }}
                onSwitchToCode={() => openFile(activeTab.path)}
                onAskAgent={handleAskAgent}
              />
            ) : (
              <CodeEditor
                path={activeTab.path}
                content={fileData?.content ?? ''}
                binary={fileData?.binary}
                theme={activeTheme}
                hasDiff={!!activeFileStatus}
                jumpToLine={activeTab.line}
                onSave={async (newContent) => {
                  await api.writeFile(activeTab.path, newContent);
                  setFileData({ content: newContent, binary: false });
                  await refreshGit();
                  showToast(`Saved ${activeTab.path}`);
                }}
                onSwitchToDiff={() => openDiff(activeTab.path)}
                onAskAgent={handleAskAgent}
              />
            )}
          </div>

          {/* Bottom Terminal */}
          {showTerminal && (
            <>
              <div
                onMouseDown={startDrag('y', (dy) =>
                  setTerminalHeight((h) => Math.max(120, Math.min(520, h - dy)))
                )}
                className="h-1 cursor-row-resize hover:bg-[var(--accent)]/60 shrink-0"
              />
              <div style={{ height: terminalHeight }} className="shrink-0">
                <TerminalPanel
                  theme={activeTheme}
                  workspacePath={workspace.path}
                  pendingCommand={pendingCmd}
                  onSendOutputToAgent={handleAskAgent}
                  onClosePanel={() => setShowTerminal(false)}
                />
              </div>
            </>
          )}
        </div>

        {/* Right Agent Panel */}
        {showAgent && (
          <>
            <div
              onMouseDown={startDrag('x', (dx) =>
                setAgentWidth((w) => Math.max(260, Math.min(620, w - dx)))
              )}
              className="w-1 cursor-col-resize hover:bg-[var(--accent)]/60 shrink-0"
            />
            <div style={{ width: agentWidth }} className="shrink-0 h-full">
              <AgentPanel
                theme={activeTheme}
                workspacePath={workspace.path}
                draftAppend={agentDraftAppend}
                onOpenDiff={(p) => openDiff(p)}
                onRefreshGit={refreshGit}
                onToast={showToast}
              />
            </div>
          </>
        )}
      </div>

      {/* Bottom Status Bar */}
      <footer
        className="h-6 px-3 flex items-center justify-between text-[11px] shrink-0 select-none"
        style={{
          background: 'var(--status-bar-bg)',
          color: 'var(--status-bar-fg)',
        }}
      >
        <div className="flex items-center gap-3">
          <span className="flex items-center gap-1 font-medium">
            <GitBranch size={12} />
            {gitStatus?.is_repo ? gitStatus.branch || 'HEAD' : 'No Git'}
          </span>
          {gitStatus?.is_repo && (
            <span>
              {gitStatus.files.length} changed
              {gitStatus.total_additions > 0 || gitStatus.total_deletions > 0
                ? ` (+${gitStatus.total_additions} -${gitStatus.total_deletions})`
                : ''}
            </span>
          )}
          <span className="flex items-center gap-1 opacity-90">
            <Radio size={11} className={liveConnected ? 'animate-pulse' : 'opacity-40'} />
            {liveConnected ? 'Live Diff Sync' : 'Reconnecting'}
          </span>
        </div>

        <div className="flex items-center gap-3 opacity-90">
          <span>{activeTheme.name}</span>
          <span className="hidden sm:inline">Ctrl+P Quick Open · Ctrl+` Terminal</span>
        </div>
      </footer>

      {/* Toast Notification */}
      {toast && (
        <div
          className="fixed bottom-9 right-4 z-50 px-3 py-2 rounded-md border shadow-lg text-xs font-medium"
          style={{
            background: toast.isError ? 'var(--danger)' : 'var(--bg-elevated)',
            borderColor: 'var(--border)',
            color: toast.isError ? '#ffffff' : 'var(--text-primary)',
          }}
        >
          {toast.msg}
        </div>
      )}

      {/* Quick Open Modal */}
      <QuickOpen
        open={quickOpenVisible}
        files={allFiles}
        themes={themes}
        tasks={tasks}
        onClose={() => setQuickOpenVisible(false)}
        onSelectFile={(p) => openFile(p)}
        onSelectTheme={(id) => setThemeId(id)}
        onRunTask={handleRunTask}
      />
    </div>
  );
}

export default App;
