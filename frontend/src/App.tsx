import React, { Suspense, lazy, useCallback, useEffect, useRef, useState } from 'react';
import {
  FolderGit2,
  FolderOpen,
  GitBranch,
  GitCompare,
  Minus,
  Palette,
  PanelBottom,
  PanelLeft,
  PanelRight,
  Play,
  Plus,
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
import { AgentPanel } from './components/AgentPanel';
import { VireoLogo } from './components/VireoLogo';

const DiffViewer = lazy(() =>
  import('./components/DiffViewer').then((m) => ({ default: m.DiffViewer }))
);
const CodeEditor = lazy(() =>
  import('./components/CodeEditor').then((m) => ({ default: m.CodeEditor }))
);
const TerminalPanel = lazy(() =>
  import('./components/TerminalPanel').then((m) => ({ default: m.TerminalPanel }))
);
const QuickOpen = lazy(() =>
  import('./components/QuickOpen').then((m) => ({ default: m.QuickOpen }))
);

const ZOOM_MIN = 0.7;
const ZOOM_MAX = 2.0;
const ZOOM_STEP = 0.1;

function clampZoom(z: number): number {
  const rounded = Math.round(z * 10) / 10;
  return Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, rounded));
}

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
  const [sidebarWidth, setSidebarWidth] = useState(300);
  const [agentWidth, setAgentWidth] = useState(400);
  const [terminalHeight, setTerminalHeight] = useState(240);

  // Zoom (persisted, synced to native Tauri WebView2 zoom with CSS fallback)
  const [zoom, setZoom] = useState<number>(() => {
    try {
      const saved = parseFloat(localStorage.getItem('vireo-zoom') || '1');
      if (Number.isFinite(saved)) return clampZoom(saved);
    } catch {
      // ignore
    }
    return 1.0;
  });

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

  const reloadActiveContentRef = useRef(reloadActiveContent);
  useEffect(() => {
    reloadActiveContentRef.current = reloadActiveContent;
  }, [reloadActiveContent]);

  // Apply zoom to native Tauri WebView2 (or fallback to CSS zoom in browser mode)
  useEffect(() => {
    let cancelled = false;
    try {
      localStorage.setItem('vireo-zoom', String(zoom));
    } catch {
      // ignore
    }
    api
      .setWindowZoom(zoom)
      .then((res) => {
        if (cancelled) return;
        if (res.native) {
          document.documentElement.style.zoom = '';
        } else {
          document.documentElement.style.zoom = String(zoom);
        }
        requestAnimationFrame(() => window.dispatchEvent(new Event('resize')));
      })
      .catch(() => {
        if (cancelled) return;
        document.documentElement.style.zoom = String(zoom);
        requestAnimationFrame(() => window.dispatchEvent(new Event('resize')));
      });
    return () => {
      cancelled = true;
    };
  }, [zoom]);

  const adjustZoom = useCallback(
    (nextOrFn: number | ((cur: number) => number)) => {
      setZoom((cur) => {
        const next = clampZoom(typeof nextOrFn === 'function' ? nextOrFn(cur) : nextOrFn);
        if (next !== cur) {
          showToast(`Zoom ${Math.round(next * 100)}%`);
        }
        return next;
      });
    },
    [showToast]
  );

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
            reloadActiveContentRef.current();
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
  }, [refreshGit, loadWorkspaceData]);

  // Global VS Code keyboard & zoom shortcuts
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      if (!mod) return;

      if (e.key === '=' || e.key === '+' || e.code === 'Equal' || e.code === 'NumpadAdd') {
        e.preventDefault();
        e.stopPropagation();
        adjustZoom((z) => z + ZOOM_STEP);
      } else if (
        e.key === '-' ||
        e.key === '_' ||
        e.code === 'Minus' ||
        e.code === 'NumpadSubtract'
      ) {
        e.preventDefault();
        e.stopPropagation();
        adjustZoom((z) => z - ZOOM_STEP);
      } else if (e.key === '0' || e.code === 'Digit0' || e.code === 'Numpad0') {
        e.preventDefault();
        e.stopPropagation();
        adjustZoom(1.0);
      } else if (e.key.toLowerCase() === 'p') {
        e.preventDefault();
        setQuickOpenVisible((v) => !v);
      } else if (e.shiftKey && e.key.toLowerCase() === 'f') {
        e.preventDefault();
        setShowSidebar(true);
        setSidebarTab('search');
      } else if (e.key === '`') {
        e.preventDefault();
        setShowTerminal((v) => !v);
      } else if (e.key.toLowerCase() === 'b') {
        e.preventDefault();
        setShowSidebar((v) => !v);
      }
    };

    const onWheel = (e: WheelEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      e.preventDefault();
      adjustZoom((z) => z + (e.deltaY < 0 ? ZOOM_STEP : -ZOOM_STEP));
    };

    window.addEventListener('keydown', onKey, { capture: true });
    window.addEventListener('wheel', onWheel, { passive: false, capture: true });
    return () => {
      window.removeEventListener('keydown', onKey, { capture: true });
      window.removeEventListener('wheel', onWheel, { capture: true });
    };
  }, [adjustZoom]);

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

  const handlePickWorkspace = async () => {
    try {
      const info = await api.pickWorkspace();
      if (!info) return;
      setWorkspace(info);
      setWsDraft(info.path);
      setEditingWs(false);
      setTabs([]);
      setActiveTabPath(null);
      await loadWorkspaceData();
      showToast(`Opened workspace: ${info.name}`);
    } catch (err: unknown) {
      showToast(err instanceof Error ? err.message : 'Failed to open folder', true);
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
      <header className="h-12 px-3.5 grid grid-cols-[1fr_auto_1fr] items-center border-b border-[var(--border)] bg-[var(--bg-activity)] shrink-0 gap-3 select-none">
        {/* Left: Brand & Workspace Path */}
        <div className="flex items-center gap-2.5 min-w-0">
          <div className="flex items-center gap-2 font-semibold text-sm text-[var(--text-primary)] shrink-0">
            <VireoLogo size={24} className="shrink-0 drop-shadow-sm" />
            <span className="tracking-tight">Vireo</span>
          </div>

          <div className="h-4 w-[1px] bg-[var(--border)] shrink-0" />

          {editingWs ? (
            <form onSubmit={handleSwitchWorkspace} className="flex items-center gap-1.5 min-w-0">
              <input
                value={wsDraft}
                onChange={(e) => setWsDraft(e.target.value)}
                onBlur={() => setEditingWs(false)}
                autoFocus
                className="w-72 h-8 px-2.5 text-[13px] font-mono rounded-md bg-[var(--bg-input)] border border-[var(--accent)] text-[var(--text-primary)] outline-none"
                placeholder="C:\path\to\repo"
              />
            </form>
          ) : (
            <div className="flex items-center gap-1.5 min-w-0">
              <button
                onClick={() => {
                  setWsDraft(workspace.path);
                  setEditingWs(true);
                }}
                className="flex items-center gap-1.5 h-8 px-2.5 rounded-md hover:bg-[var(--bg-hover)] text-[13px] text-[var(--text-primary)] max-w-[220px] cursor-pointer transition-colors"
                title={`Workspace: ${workspace.path} (click to edit path)`}
              >
                <FolderGit2 size={15} className="text-[var(--accent)] shrink-0" />
                <span className="truncate font-medium">{workspace.name}</span>
              </button>
              <button
                onClick={handlePickWorkspace}
                className="flex items-center gap-1.5 h-8 px-2.5 rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[12.5px] text-[var(--text-primary)] shrink-0 cursor-pointer transition-colors"
                title="Open Folder (Native Dialog)"
              >
                <FolderOpen size={14} className="text-[var(--accent)]" />
                <span className="hidden lg:inline">Open Folder</span>
              </button>
            </div>
          )}
        </div>

        {/* Center: True-Centered Quick Open Bar */}
        <button
          onClick={() => setQuickOpenVisible(true)}
          className="hidden md:flex items-center gap-2.5 h-8 px-3.5 w-[340px] lg:w-[400px] rounded-lg border border-[var(--border)] bg-[var(--bg-input)] hover:border-[var(--accent)] text-[13px] text-[var(--text-dim)] cursor-pointer transition-colors"
        >
          <Search size={14} className="shrink-0" />
          <span className="flex-1 text-left truncate">
            Quick Open files, themes, tasks...
          </span>
          <kbd className="text-[11px] font-mono px-1.5 py-0.5 rounded bg-[var(--bg-elevated)] border border-[var(--border)] text-[var(--text-muted)]">
            Ctrl+P
          </kbd>
        </button>

        {/* Right: Run Tasks, Themes, Pane Toggles */}
        <div className="flex items-center justify-end gap-2 shrink-0">
          {/* Run Scripts Dropdown */}
          <div className="relative">
            <button
              onClick={() => {
                setShowRunMenu((v) => !v);
                setShowThemeMenu(false);
              }}
              className="h-8 px-2.5 text-[12.5px] rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] flex items-center gap-1.5 cursor-pointer transition-colors"
              title="Run detected project scripts"
            >
              <Play size={13} className="text-[var(--added-fg)]" />
              Run
            </button>
            {showRunMenu && (
              <div className="absolute right-0 mt-1.5 w-68 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-xl z-40 py-1.5">
                <div className="px-3 py-1 text-[11px] uppercase font-semibold tracking-wider text-[var(--text-dim)]">
                  Detected Project Tasks
                </div>
                {tasks.length === 0 ? (
                  <div className="px-3 py-2 text-[13px] text-[var(--text-dim)]">
                    No package.json / Cargo.toml tasks found.
                  </div>
                ) : (
                  tasks.map((t) => (
                    <button
                      key={`${t.source}-${t.name}`}
                      onClick={() => handleRunTask(t)}
                      className="w-full text-left px-3 py-2 hover:bg-[var(--bg-hover)] flex flex-col cursor-pointer"
                    >
                      <span className="text-[13px] text-[var(--text-primary)]">{t.name}</span>
                      <span className="text-[11px] font-mono text-[var(--text-dim)] truncate">
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
              className="h-8 px-2.5 text-[12.5px] rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] flex items-center gap-1.5 cursor-pointer transition-colors"
              title="Switch theme or import a VS Code JSON theme"
            >
              <Palette size={14} className="text-[var(--accent)]" />
              <span className="hidden sm:inline">{activeTheme.name}</span>
            </button>
            {showThemeMenu && (
              <div className="absolute right-0 mt-1.5 w-60 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-xl z-40 py-1.5">
                <div className="px-3 py-1 text-[11px] uppercase font-semibold tracking-wider text-[var(--text-dim)]">
                  Color Themes
                </div>
                {themes.map((th) => (
                  <button
                    key={th.id}
                    onClick={() => {
                      setThemeId(th.id);
                      setShowThemeMenu(false);
                    }}
                    className={`w-full text-left px-3 py-2 text-[13px] flex items-center justify-between cursor-pointer ${
                      th.id === activeTheme.id
                        ? 'bg-[var(--accent)] text-[var(--accent-fg)]'
                        : 'hover:bg-[var(--bg-hover)] text-[var(--text-primary)]'
                    }`}
                  >
                    <span>{th.name}</span>
                    <span
                      className="w-3.5 h-3.5 rounded-full border border-white/25"
                      style={{ background: th.vars['--bg-base'] }}
                    />
                  </button>
                ))}
                <div className="border-t border-[var(--border)] mt-1.5 pt-1.5 px-2">
                  <button
                    onClick={() => themeFileInputRef.current?.click()}
                    className="w-full py-1.5 px-2 text-[12.5px] rounded hover:bg-[var(--bg-hover)] text-[var(--accent)] flex items-center gap-1.5 cursor-pointer"
                  >
                    <Upload size={13} />
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
          <div className="flex items-center gap-1">
            <button
              onClick={() => setShowSidebar((v) => !v)}
              className={`w-8 h-8 flex items-center justify-center rounded-md cursor-pointer transition-colors ${
                showSidebar
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                  : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
              }`}
              title="Toggle Sidebar (Ctrl+B)"
            >
              <PanelLeft size={16} />
            </button>
            <button
              onClick={() => setShowTerminal((v) => !v)}
              className={`w-8 h-8 flex items-center justify-center rounded-md cursor-pointer transition-colors ${
                showTerminal
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                  : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
              }`}
              title="Toggle Terminal (Ctrl+`)"
            >
              <PanelBottom size={16} />
            </button>
            <button
              onClick={() => setShowAgent((v) => !v)}
              className={`w-8 h-8 flex items-center justify-center rounded-md cursor-pointer transition-colors ${
                showAgent
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                  : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
              }`}
              title="Toggle Agent Panel"
            >
              <PanelRight size={16} />
            </button>
          </div>
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
            <div className="flex items-center h-10 bg-[var(--bg-sidebar)] border-b border-[var(--border)] overflow-x-auto shrink-0">
              {tabs.map((tab) => {
                const active = tab.path === activeTabPath;
                const name = tab.path.split('/').pop() || tab.path;
                return (
                  <div
                    key={tab.path}
                    onClick={() => setActiveTabPath(tab.path)}
                    className={`group flex items-center gap-2 px-3.5 h-full text-[13px] border-r border-[var(--border)] cursor-pointer select-none shrink-0 transition-colors ${
                      active
                        ? 'bg-[var(--bg-base)] text-[var(--text-primary)] font-medium border-t-2 border-t-[var(--accent)]'
                        : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                    }`}
                  >
                    {tab.mode === 'diff' ? (
                      <GitCompare size={14} className="text-[var(--modified-fg)] shrink-0" />
                    ) : (
                      <FileCode size={14} className="text-[var(--text-muted)] shrink-0" />
                    )}
                    <span>{name}</span>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        closeTab(tab.path);
                      }}
                      className="ml-0.5 p-0.5 rounded hover:bg-[var(--bg-hover)] hover:text-[var(--danger)] cursor-pointer"
                      title="Close tab"
                    >
                      <X size={13} />
                    </button>
                  </div>
                );
              })}
            </div>
          )}

          {/* Main Viewer Stage */}
          <div className="flex-1 flex flex-col min-h-0">
            {!activeTab ? (
              <div className="flex-1 flex flex-col items-center justify-center p-8 text-center gap-5">
                <VireoLogo size={56} className="drop-shadow-lg" />
                <div className="space-y-1.5 max-w-md">
                  <h2 className="text-lg font-semibold tracking-tight text-[var(--text-primary)]">
                    Vireo — Agent Review Workbench
                  </h2>
                  <p className="text-[13.5px] text-[var(--text-muted)] leading-relaxed">
                    Ultra-lightweight workspace for reviewing live agent diffs, running commands,
                    and shipping commits to GitHub without VS Code overhead.
                  </p>
                </div>
                <div className="flex flex-wrap items-center justify-center gap-2.5 text-[13px]">
                  <button
                    onClick={() => setQuickOpenVisible(true)}
                    className="px-3.5 py-2 rounded-lg bg-[var(--accent)] text-[var(--accent-fg)] font-medium cursor-pointer hover:opacity-95 transition-opacity"
                  >
                    Quick Open (Ctrl+P)
                  </button>
                  <button
                    onClick={() => {
                      setShowSidebar(true);
                      setSidebarTab('explorer');
                    }}
                    className="px-3.5 py-2 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                  >
                    Browse Files
                  </button>
                  <button
                    onClick={handlePickWorkspace}
                    className="px-3.5 py-2 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] flex items-center gap-1.5 cursor-pointer transition-colors"
                  >
                    <FolderOpen size={15} className="text-[var(--accent)]" />
                    Open Folder
                  </button>
                  <button
                    onClick={() => setShowTerminal(true)}
                    className="px-3.5 py-2 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                  >
                    Open Terminal (Ctrl+`)
                  </button>
                </div>
              </div>
            ) : activeTab.mode === 'diff' ? (
              <Suspense
                fallback={
                  <div className="flex-1 flex items-center justify-center text-xs text-[var(--text-dim)]">
                    Loading diff viewer...
                  </div>
                }
              >
                <DiffViewer
                  diff={diffData}
                  fileStatus={activeFileStatus}
                  theme={activeTheme}
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
              </Suspense>
            ) : (
              <Suspense
                fallback={
                  <div className="flex-1 flex items-center justify-center text-xs text-[var(--text-dim)]">
                    Loading editor...
                  </div>
                }
              >
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
              </Suspense>
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
                <Suspense
                  fallback={
                    <div className="h-full bg-[var(--bg-panel)] border-t border-[var(--border)] flex items-center justify-center text-xs text-[var(--text-dim)]">
                      Loading terminal...
                    </div>
                  }
                >
                  <TerminalPanel
                    theme={activeTheme}
                    workspacePath={workspace.path}
                    pendingCommand={pendingCmd}
                    onSendOutputToAgent={handleAskAgent}
                    onClosePanel={() => setShowTerminal(false)}
                  />
                </Suspense>
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
        className="h-7 px-3.5 flex items-center justify-between text-xs border-t border-[var(--border)] shrink-0 select-none"
        style={{
          background: 'var(--status-bar-bg)',
          color: 'var(--status-bar-fg)',
        }}
      >
        <div className="flex items-center gap-3.5">
          <span className="flex items-center gap-1.5 font-medium">
            <GitBranch size={13} />
            {gitStatus?.is_repo ? gitStatus.branch || 'HEAD' : 'No Git'}
          </span>
          {gitStatus?.is_repo && (
            <span className="tabular-nums text-[var(--text-muted)]">
              {gitStatus.files.length} changed
              {gitStatus.total_additions > 0 || gitStatus.total_deletions > 0
                ? ` (+${gitStatus.total_additions} -${gitStatus.total_deletions})`
                : ''}
            </span>
          )}
          <span className="flex items-center gap-1.5 text-[var(--text-muted)]">
            <span
              className="w-2 h-2 rounded-full"
              style={{
                background: liveConnected ? '#10b981' : '#f59e0b',
              }}
            />
            {liveConnected ? 'Live Diff Sync' : 'Reconnecting'}
          </span>
        </div>

        <div className="flex items-center gap-3.5">
          {/* Interactive Zoom Controls */}
          <div
            className="flex items-center gap-0.5 rounded px-1 py-0.5 bg-[var(--bg-elevated)]/60 border border-[var(--border)]/70"
            title="Adjust UI Zoom (Ctrl+= / Ctrl+- / Ctrl+0)"
          >
            <button
              onClick={() => adjustZoom((z) => z - ZOOM_STEP)}
              className="p-0.5 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
              title="Zoom Out (Ctrl+-)"
            >
              <Minus size={11} />
            </button>
            <button
              onClick={() => adjustZoom(1.0)}
              className="px-1.5 text-[11px] font-mono tabular-nums text-[var(--text-primary)] hover:text-[var(--accent)] cursor-pointer"
              title="Reset Zoom to 100% (Ctrl+0)"
            >
              {Math.round(zoom * 100)}%
            </button>
            <button
              onClick={() => adjustZoom((z) => z + ZOOM_STEP)}
              className="p-0.5 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
              title="Zoom In (Ctrl+=)"
            >
              <Plus size={11} />
            </button>
          </div>

          <span className="text-[var(--text-muted)]">{activeTheme.name}</span>
          <span className="hidden md:inline text-[var(--text-dim)]">
            Ctrl+P Quick Open · Ctrl+` Terminal · Ctrl+± Zoom
          </span>
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
      {quickOpenVisible && (
        <Suspense fallback={null}>
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
        </Suspense>
      )}
    </div>
  );
}

export default App;
