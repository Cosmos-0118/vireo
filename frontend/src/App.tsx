import React, {
  Suspense,
  lazy,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import {
  Check,
  Crosshair,
  FolderGit2,
  FolderOpen,
  GitBranch,
  GitCommitHorizontal,
  GitCompare,
  GitFork,
  Minus,
  Palette,
  PanelBottom,
  PanelLeft,
  PanelRight,
  Play,
  RotateCcw,
  Plus,
  Search,
  Upload,
  X,
  FileCode,
} from 'lucide-react';
import { diffLines } from 'diff';
import { api, wsUrl } from './api';
import {
  applyTheme,
  BUILTIN_THEMES,
  LiteTheme,
  parseVsCodeThemeJson,
} from './themes';
import type {
  CommitFileChange,
  GitDiffRes,
  GitStatusRes,
  OpenTab,
  TaskItem,
  ToastAction,
  WorkspaceInfo,
} from './types';
import { Sidebar, SidebarTab } from './components/Sidebar';
import { AgentPanel } from './components/AgentPanel';
import { AgentEditsStrip } from './components/AgentEditsStrip';
import { FileIcon } from './components/FileIcon';
import { VireoLogo } from './components/VireoLogo';
import ResizeHandle from './components/ResizeHandle';
import {
  AGENT,
  SIDEBAR,
  TERMINAL,
  loadLayout,
  saveLayout,
  solveTerminalHeight,
  solveWidths,
  type PanelSide,
} from './layout';

const DiffViewer = lazy(() =>
  import('./components/DiffViewer').then((m) => ({ default: m.DiffViewer }))
);
const CommitGraphView = lazy(() =>
  import('./components/CommitGraphView').then((m) => ({ default: m.CommitGraphView }))
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
  const [fsVersion, setFsVersion] = useState(0);
  const [allFiles, setAllFiles] = useState<string[]>([]);
  const [tasks, setTasks] = useState<TaskItem[]>([]);
  const [liveConnected, setLiveConnected] = useState(false);

  // Native window state for the custom title bar (null = running in a browser)
  const [nativeWindow, setNativeWindow] = useState<{ maximized: boolean } | null>(null);
  const windowAction = useCallback(
    (action: 'state' | 'minimize' | 'toggle_maximize' | 'close' | 'drag') => {
      api
        .windowControl(action)
        .then((res) => setNativeWindow(res.native ? { maximized: res.maximized } : null))
        .catch(() => {});
    },
    []
  );
  useEffect(() => {
    windowAction('state');
    let timer: number | undefined;
    const onResize = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => windowAction('state'), 120);
    };
    window.addEventListener('resize', onResize);
    return () => {
      window.removeEventListener('resize', onResize);
      window.clearTimeout(timer);
    };
  }, [windowAction]);

  const handleTitleBarMouseDown = (e: React.MouseEvent) => {
    if (!nativeWindow || e.button !== 0) return;
    const target = e.target as HTMLElement;
    if (target.closest('button, input, textarea, select, a, kbd, form, [data-no-drag]')) return;
    e.preventDefault();
    windowAction(e.detail === 2 ? 'toggle_maximize' : 'drag');
  };

  // Layout toggles & sizes
  const [showSidebar, setShowSidebar] = useState(true);
  const [showAgent, setShowAgent] = useState(true);
  const [showTerminal, setShowTerminal] = useState(true);
  const [sidebarTab, setSidebarTab] = useState<SidebarTab>('changes');
  // Preferred (user-chosen, persisted) sizes. What is rendered is derived below from the
  // measured workbench so a small window never destroys the stored layout.
  const [storedLayout] = useState(loadLayout);
  const [sidebarWidth, setSidebarWidth] = useState(storedLayout.sidebarWidth);
  const [agentWidth, setAgentWidth] = useState(storedLayout.agentWidth);
  const [terminalHeight, setTerminalHeight] = useState(storedLayout.terminalHeight);
  const [terminalMaximized, setTerminalMaximized] = useState(false);
  const [launcherSignal, setLauncherSignal] = useState(0);
  const [panelPriority, setPanelPriority] = useState<PanelSide>('sidebar');
  const [workbenchSize, setWorkbenchSize] = useState({ w: 0, h: 0 });
  const workbenchRef = useRef<HTMLDivElement>(null);
  const [graphSelectedHash, setGraphSelectedHash] = useState<string | null>(null);

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
  const [toast, setToast] = useState<{ msg: string; isError?: boolean; action?: ToastAction } | null>(null);

  // Tabs & Active content
  const [tabs, setTabs] = useState<OpenTab[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);
  const [diffData, setDiffData] = useState<GitDiffRes | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [fileData, setFileData] = useState<{
    path: string;
    content: string;
    binary: boolean;
  } | null>(null);

  // Live agent edits (driven by the file watcher, so it works for any agent or tool)
  const [followEdits, setFollowEdits] = useState<boolean>(() => {
    try {
      return localStorage.getItem('vireo-follow-edits') !== '0';
    } catch {
      return true;
    }
  });
  const [liveEdits, setLiveEdits] = useState<
    Array<{ path: string; adds: number; dels: number; at: number }>
  >([]);
  const [pillVisible, setPillVisible] = useState(false);
  const [pausedJump, setPausedJump] = useState<string | null>(null);
  const [liveFlash, setLiveFlash] = useState<{
    path: string;
    key: number;
    ranges: Array<{ from: number; to: number }>;
  } | null>(null);
  /** path -> pulse key; drives the short green pulse on tabs and explorer rows. */
  const [tabPulse, setTabPulse] = useState<Record<string, number>>({});
  /** path -> content before the agent's unreviewed edits (drives inline Keep / Undo). */
  const [review, setReview] = useState<Record<string, string>>({});
  const [pillDismissed, setPillDismissed] = useState(false);
  const [undoConfirm, setUndoConfirm] = useState(false);
  const userActivityRef = useRef(0);
  /** Last loaded content per tab, shown instantly on switch while a fresh copy loads. */
  const contentCacheRef = useRef(
    new Map<
      string,
      { diff?: GitDiffRes; file?: { path: string; content: string; binary: boolean } }
    >()
  );
  const activeTabIdRef = useRef<string | null>(null);
  const reviewRef = useRef<Record<string, string>>({});
  const editorDirtyRef = useRef(false);
  /** Last known on-disk content per path; the baseline for "what did the agent just change". */
  const snapshotsRef = useRef<Map<string, string>>(new Map());
  const inflightRef = useRef<Set<string>>(new Set());
  const selfWriteRef = useRef<{ all: number; paths: Map<string, number> }>({
    all: 0,
    paths: new Map(),
  });
  const fsChainRef = useRef<Promise<void>>(Promise.resolve());
  const fsChangedRef = useRef<(paths: string[], dirtyBefore: Set<string>) => void>(() => {});
  const flashKeyRef = useRef(0);
  const pillTimerRef = useRef<number | undefined>(undefined);
  const tabsRef = useRef<OpenTab[]>([]);
  const gitStatusRef = useRef<GitStatusRes | null>(null);
  const followRef = useRef(followEdits);

  /** Our own writes (save, discard, revert) must not be announced as agent edits. */
  const markSelfWrite = useCallback((path: string | null) => {
    const until = Date.now() + 1500;
    if (path) {
      selfWriteRef.current.paths.set(path, until);
      snapshotsRef.current.delete(path);
    } else {
      selfWriteRef.current.all = until;
    }
  }, []);

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

  const showToast = useCallback((msg: string, isError = false, action?: ToastAction) => {
    setToast({ msg, isError, action });
    setTimeout(() => setToast((cur) => (cur?.msg === msg ? null : cur)), action ? 10000 : 3600);
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
          setActiveTabId(first.path);
          return [
            {
              id: first.path,
              path: first.path,
              mode: 'diff',
              staged: first.staged && !first.unstaged,
            },
          ];
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
  const activeTab = tabs.find((t) => t.id === activeTabId) || null;

  activeTabIdRef.current = activeTabId;

  const reloadActiveContent = useCallback(async () => {
    if (!activeTab) {
      setDiffData(null);
      setFileData(null);
      return;
    }
    if (activeTab.mode === 'graph') {
      return;
    }
    const id = activeTab.id;
    const cache = contentCacheRef.current;
    const isCurrent = () => activeTabIdRef.current === id;
    const remember = (patch: { diff?: GitDiffRes; file?: { path: string; content: string; binary: boolean } }) => {
      const prev = cache.get(id);
      cache.delete(id);
      cache.set(id, { ...prev, ...patch });
      while (cache.size > 30) cache.delete(cache.keys().next().value as string);
    };
    const sameDiff = (a: GitDiffRes | null, b: GitDiffRes) =>
      !!a &&
      a.path === b.path &&
      a.staged === b.staged &&
      a.binary === b.binary &&
      a.modified === b.modified &&
      a.original === b.original;

    let loadDiff: (() => Promise<GitDiffRes>) | null = null;
    if (activeTab.mode === 'diff') {
      loadDiff = () => api.gitDiff(activeTab.path, !!activeTab.staged);
    } else if (activeTab.mode === 'commit-diff' && activeTab.commitHash) {
      const hash = activeTab.commitHash;
      loadDiff = () => api.gitCommitDiff(hash, activeTab.path, activeTab.oldPath);
    } else if (activeTab.mode === 'compare-diff' && activeTab.commitHash) {
      const hash = activeTab.commitHash;
      loadDiff = () => api.gitCompareDiff(hash, activeTab.path, activeTab.oldPath);
    }

    if (loadDiff) {
      // Cached copy is already on screen: revalidate silently, no loading flash.
      if (!cache.get(id)?.diff) setDiffLoading(true);
      try {
        const d = await loadDiff();
        if (
          activeTab.mode === 'diff' &&
          !d.binary &&
          !activeTab.staged &&
          !snapshotsRef.current.has(activeTab.path) &&
          !inflightRef.current.has(activeTab.path)
        ) {
          snapshotsRef.current.set(activeTab.path, d.modified);
        }
        remember({ diff: d });
        // Keep the same object when nothing changed so the viewer does no recompute.
        if (isCurrent()) setDiffData((prev) => (sameDiff(prev, d) ? prev : d));
      } catch {
        if (isCurrent()) setDiffData(null);
      } finally {
        if (isCurrent()) setDiffLoading(false);
      }
    } else {
      try {
        const f = await api.readFile(activeTab.path);
        if (
          !f.binary &&
          !snapshotsRef.current.has(activeTab.path) &&
          !inflightRef.current.has(activeTab.path)
        ) {
          snapshotsRef.current.set(activeTab.path, f.content);
        }
        const fd = { path: activeTab.path, content: f.content, binary: f.binary };
        remember({ file: fd });
        if (isCurrent()) {
          setFileData((prev) =>
            prev && prev.path === fd.path && prev.content === fd.content && prev.binary === fd.binary
              ? prev
              : fd
          );
        }
      } catch {
        if (isCurrent()) setFileData(null);
      }
    }
  }, [activeTab]);

  // Switching tabs: paint the cached content immediately (before the browser paints), then revalidate.
  useLayoutEffect(() => {
    const c = activeTabId ? contentCacheRef.current.get(activeTabId) : undefined;
    setDiffData(c?.diff ?? null);
    setFileData(c?.file ?? null);
    const diffMode =
      activeTab?.mode === 'diff' || activeTab?.mode === 'commit-diff' || activeTab?.mode === 'compare-diff';
    setDiffLoading(diffMode && !c?.diff);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTabId, activeTab?.mode]);

  // Warm the viewer chunks so the first switch to a diff / editor tab does not wait on a download.
  useEffect(() => {
    const t = window.setTimeout(() => {
      void import('./components/DiffViewer');
      void import('./components/CodeEditor');
    }, 1200);
    return () => window.clearTimeout(t);
  }, []);

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
            fsChangedRef.current(
              Array.isArray(data.paths) ? data.paths : [],
              new Set(gitStatusRef.current?.files.map((f) => f.path) ?? [])
            );
            setFsVersion((v) => v + 1);
            refreshGit();
            reloadActiveContentRef.current();
          } else if (data.type === 'workspace_switched') {
            snapshotsRef.current.clear();
            contentCacheRef.current.clear();
            setLiveEdits([]);
            setReview({});
            setPillDismissed(false);
            setTabPulse({});
            setPausedJump(null);
            setLiveFlash(null);
            setPillVisible(false);
            setTabs([]);
            setActiveTabId(null);
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

      const inXterm = !!(e.target as HTMLElement | null)?.closest?.('.xterm');

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
      } else if (e.key.toLowerCase() === 'p' && !inXterm) {
        e.preventDefault();
        setQuickOpenVisible((v) => !v);
      } else if (e.shiftKey && e.key.toLowerCase() === 'f') {
        e.preventDefault();
        setShowSidebar(true);
        setSidebarTab('search');
      } else if (e.shiftKey && e.key.toLowerCase() === 'g' && !inXterm) {
        e.preventDefault();
        setShowSidebar(true);
        setSidebarTab('changes');
      } else if (e.key === '`') {
        e.preventDefault();
        setShowTerminal((v) => !v);
      } else if (e.key.toLowerCase() === 'b' && !inXterm) {
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
      const existing = prev.find((t) => t.id === path);
      if (existing) {
        return prev.map((t) => (t.id === path ? { ...t, mode: 'diff', staged } : t));
      }
      return [...prev, { id: path, path, mode: 'diff', staged }];
    });
    setActiveTabId(path);
  }, []);

  const openFile = useCallback((path: string, line?: number, col?: number, colEnd?: number) => {
    const jumpKey = Date.now();
    setTabs((prev) => {
      const existing = prev.find((t) => t.id === path);
      if (existing) {
        return prev.map((t) =>
          t.id === path ? { ...t, mode: 'code', line, col, colEnd, jumpKey } : t
        );
      }
      return [...prev, { id: path, path, mode: 'code', line, col, colEnd, jumpKey }];
    });
    setActiveTabId(path);
  }, []);

  tabsRef.current = tabs;
  reviewRef.current = review;
  gitStatusRef.current = gitStatus;
  followRef.current = followEdits;

  const focusPath = useCallback(
    (path: string) => {
      const existing = tabsRef.current.find(
        (t) => t.path === path && (t.mode === 'diff' || t.mode === 'code')
      );
      if (existing) setActiveTabId(existing.id);
      else openFile(path);
      setPausedJump(null);
    },
    [openDiff, openFile]
  );

  // Watcher batch -> "what did the agent just change?" (works for any agent, CLI or built-in).
  fsChangedRef.current = (paths: string[], dirtyBefore: Set<string>) => {
    const lock =
      /(^|\/)(package-lock\.json|yarn\.lock|pnpm-lock\.yaml|Cargo\.lock|bun\.lockb?|composer\.lock|poetry\.lock|Gemfile\.lock)$/;
    let files = paths
      .map((p) => (p || '').replace(/\\/g, '/').replace(/^\.\//, ''))
      .filter((p) => p && !p.includes(':') && !p.startsWith('.git/') && p !== 'cmd' && !p.endsWith('/'));
    const source = files.filter((p) => !lock.test(p));
    if (source.length) files = source;
    files = files.slice(0, 8);
    if (!files.length) return;
    files.forEach((p) => inflightRef.current.add(p));

    const run = async () => {
      const results: Array<{
        path: string;
        adds: number;
        dels: number;
        ranges: Array<{ from: number; to: number }>;
        /** Content before this edit, or null when it is not a trustworthy review baseline. */
        baseline: string | null;
        /** Net change vs the review baseline (what the sidebar shows), when a baseline is known. */
        total: { adds: number; dels: number } | null;
      }> = [];
      const currents = new Map<string, string>();
      for (const path of files) {
        let cur: string;
        try {
          const f = await api.readFile(path);
          if (f.binary || f.size > 1_500_000) {
            snapshotsRef.current.delete(path);
            continue;
          }
          cur = f.content;
          currents.set(path, f.content);
        } catch {
          snapshotsRef.current.delete(path);
          continue;
        }
        let prev = snapshotsRef.current.get(path);
        // Baseline taken from git HEAD is only a valid "before the agent" state if the file was clean.
        let trusted = prev !== undefined || !dirtyBefore.has(path);
        if (prev === undefined) {
          try {
            const d = await api.gitDiff(path, false);
            if (!d.binary) prev = d.original;
          } catch {
            // not in a repo: no baseline
          }
        }
        const snaps = snapshotsRef.current;
        snaps.delete(path);
        snaps.set(path, cur);
        while (snaps.size > 40) snaps.delete(snaps.keys().next().value as string);
        if (prev === undefined || prev === cur) continue;

        // CRLF vs LF must not look like a whole-file rewrite (git stores LF, the working tree is CRLF).
        const lf = (t: string) => t.replace(/\r\n/g, '\n');
        let adds = 0;
        let dels = 0;
        let line = 0;
        const ranges: Array<{ from: number; to: number }> = [];
        for (const part of diffLines(lf(prev), lf(cur))) {
          const n = part.count ?? 0;
          if (part.added) {
            adds += n;
            if (n > 0) ranges.push({ from: line + 1, to: line + n });
            line += n;
          } else if (part.removed) {
            dels += n;
          } else {
            line += n;
          }
        }
        if (adds || dels) {
          const eolMatched =
            cur.includes('\r\n') && !prev.includes('\r\n') ? prev.replace(/\n/g, '\r\n') : prev;
          const base = reviewRef.current[path] ?? (trusted ? prev : null);
          let total: { adds: number; dels: number } | null = null;
          if (base !== null) {
            total = { adds: 0, dels: 0 };
            for (const part of diffLines(lf(base), lf(cur))) {
              if (part.added) total.adds += part.count ?? 0;
              else if (part.removed) total.dels += part.count ?? 0;
            }
          }
          results.push({ path, adds, dels, ranges, baseline: trusted ? eolMatched : null, total });
        }
      }

      // Files whose content is back to their review baseline are no longer pending.
      const norm = (t: string) => t.replace(/\r\n?/g, '\n');
      setReview((prev) => {
        let changed = false;
        const next = { ...prev };
        for (const [p, c] of currents) {
          if (next[p] !== undefined && norm(next[p]) === norm(c)) {
            delete next[p];
            changed = true;
          }
        }
        return changed ? next : prev;
      });

      const sup = selfWriteRef.current;
      const now = Date.now();
      const ui = results.filter((r) => !(sup.all > now || (sup.paths.get(r.path) ?? 0) > now));
      if (!ui.length) return;

      setPillDismissed(false);
      setReview((prev) => {
        let changed = false;
        const next = { ...prev };
        for (const r of ui) {
          if (r.baseline !== null && next[r.path] === undefined) {
            next[r.path] = r.baseline;
            changed = true;
          }
        }
        return changed ? next : prev;
      });

      const key = ++flashKeyRef.current;
      setLiveEdits((prev) => {
        const m = new Map(prev.map((e) => [e.path, e]));
        for (const r of ui) {
          const old = m.get(r.path);
          m.delete(r.path);
          m.set(r.path, {
            path: r.path,
            adds: r.total ? r.total.adds : (old?.adds ?? 0) + r.adds,
            dels: r.total ? r.total.dels : (old?.dels ?? 0) + r.dels,
            at: Date.now(),
          });
        }
        return [...m.values()].slice(-100);
      });
      setPillVisible(true);
      window.clearTimeout(pillTimerRef.current);
      pillTimerRef.current = window.setTimeout(() => {
        setPillVisible(false);
        setPausedJump(null);
      }, 10000);

      setTabPulse((prev) => ({ ...prev, ...Object.fromEntries(ui.map((r) => [r.path, key])) }));
      window.setTimeout(
        () =>
          setTabPulse((prev) => {
            const next = { ...prev };
            for (const r of ui) if (next[r.path] === key) delete next[r.path];
            return next;
          }),
        1800
      );

      const target = ui[ui.length - 1];
      setLiveFlash({ path: target.path, key, ranges: target.ranges });
      window.setTimeout(() => setLiveFlash((f) => (f?.key === key ? null : f)), 1650);
      if (followRef.current) {
        const busy = editorDirtyRef.current || Date.now() - userActivityRef.current < 4000;
        if (busy) setPausedJump(target.path);
        else focusPath(target.path);
      }
    };

    fsChainRef.current = fsChainRef.current
      .then(run)
      .catch(() => {})
      .finally(() => files.forEach((p) => inflightRef.current.delete(p)));
  };

  useEffect(() => {
    if (pausedJump && activeTab?.path === pausedJump) setPausedJump(null);
  }, [pausedJump, activeTab]);

  const toggleFollow = () => {
    setFollowEdits((v) => {
      const next = !v;
      try {
        localStorage.setItem('vireo-follow-edits', next ? '1' : '0');
      } catch {
        // ignore
      }
      return next;
    });
    setPausedJump(null);
  };

  const reviewPaths = Object.keys(review);

  const setReviewBaseFor = useCallback((path: string, b: string | null) => {
    setReview((prev) => {
      if (b === null) {
        if (prev[path] === undefined) return prev;
        const next = { ...prev };
        delete next[path];
        return next;
      }
      const eol = (prev[path] ?? '').includes('\r\n') ? '\r\n' : '\n';
      return { ...prev, [path]: b.replace(/\r\n?/g, '\n').replace(/\n/g, eol) };
    });
  }, []);

  // Committed, stashed or reverted files have no pending changes left: drop them from the
  // agent-edits bar and from the review baselines (so "Undo all" can't roll back committed work).
  useEffect(() => {
    if (!gitStatus?.is_repo) return;
    const dirty = gitStatus.files.map((f) => f.path.replace(/\\/g, '/'));
    const exact = new Set(dirty);
    const dirs = dirty.filter((d) => d.endsWith('/'));
    const stillChanged = (p: string) => exact.has(p) || dirs.some((d) => p.startsWith(d));
    setLiveEdits((prev) => {
      const next = prev.filter((e) => stillChanged(e.path));
      return next.length === prev.length ? prev : next;
    });
    setReview((prev) => {
      const gone = Object.keys(prev).filter((p) => !stillChanged(p));
      if (!gone.length) return prev;
      const next = { ...prev };
      for (const p of gone) delete next[p];
      return next;
    });
  }, [gitStatus]);

  const handleFileUndone = (path: string, before: string, base: string) => {
    const prevLive = liveEditsRef.current.filter((e) => e.path === path);
    showToast(`Reverted agent edits in ${path}`, false, {
      label: 'Redo',
      run: () => void redoReverted([{ path, content: before }], { [path]: base }, prevLive),
    });
  };

  const keepAll = () => {
    setReview({});
    setLiveEdits([]);
    setPillVisible(false);
    setPausedJump(null);
  };

  const liveEditsRef = useRef(liveEdits);
  liveEditsRef.current = liveEdits;

  /** Puts reverted files back exactly as they were, together with the agent-edits strip. */
  const redoReverted = async (
    saved: Array<{ path: string; content: string | null }>,
    prevReview: Record<string, string>,
    prevLive: typeof liveEdits
  ) => {
    try {
      for (const { path, content } of saved) {
        if (content === null) continue;
        markSelfWrite(path);
        await api.writeFile(path, content);
      }
      await refreshGit();
      setReview((cur) => ({ ...prevReview, ...cur }));
      setLiveEdits((cur) => [...prevLive.filter((e) => !cur.some((c) => c.path === e.path)), ...cur]);
      reloadActiveContent();
      showToast(`Re-applied agent edits in ${saved.length} file${saved.length === 1 ? '' : 's'}`);
    } catch (e: unknown) {
      showToast(e instanceof Error ? e.message : 'Failed to redo agent edits', true);
    }
  };

  const undoAll = async () => {
    const entries = Object.entries(review);
    const prevReview = review;
    const prevLive = liveEdits;
    // Keep what is on disk now so the revert can be redone without loss.
    const saved: Array<{ path: string; content: string | null }> = [];
    for (const [path] of entries) {
      try {
        const f = await api.readFile(path);
        saved.push({ path, content: f.binary ? null : f.content });
      } catch {
        saved.push({ path, content: null });
      }
    }
    try {
      for (const [path, base] of entries) {
        markSelfWrite(path);
        const st = gitStatusRef.current?.files.find((f) => f.path === path);
        if (st?.untracked && base === '') {
          await api.gitDiscard(path, true);
          snapshotsRef.current.delete(path);
        } else {
          await api.writeFile(path, base);
          snapshotsRef.current.set(path, base);
        }
      }
      setReview({});
      setLiveEdits([]);
      setPillVisible(false);
      setPausedJump(null);
      await refreshGit();
      reloadActiveContent();
      showToast(`Reverted agent edits in ${entries.length} file${entries.length === 1 ? '' : 's'}`, false, {
        label: 'Redo',
        run: () => void redoReverted(saved, prevReview, prevLive),
      });
    } catch (e: unknown) {
      showToast(e instanceof Error ? e.message : 'Failed to undo agent edits', true);
    }
  };

  const openCommitDiff = useCallback(
    (
      commit: { hash: string; short_hash: string; subject: string; author: string; date: string },
      file: CommitFileChange,
      allFiles: CommitFileChange[]
    ) => {
      const id = `commit:${commit.hash}:${file.path}`;
      const tab: OpenTab = {
        id,
        path: file.path,
        mode: 'commit-diff',
        commitHash: commit.hash,
        commitShortHash: commit.short_hash,
        commitSubject: commit.subject,
        commitAuthor: commit.author,
        commitDate: commit.date,
        oldPath: file.old_path,
        commitFiles: allFiles,
      };
      setTabs((prev) => {
        const existing = prev.find((t) => t.id === id);
        if (existing) return prev.map((t) => (t.id === id ? tab : t));
        return [...prev, tab];
      });
      setActiveTabId(id);
    },
    []
  );

  const openCompareDiff = useCallback(
    (
      commit: { hash: string; short_hash: string; subject: string },
      file: CommitFileChange,
      allFiles: CommitFileChange[]
    ) => {
      const id = `compare:${commit.hash}:${file.path}`;
      const tab: OpenTab = {
        id,
        path: file.path,
        mode: 'compare-diff',
        commitHash: commit.hash,
        commitShortHash: commit.short_hash,
        commitSubject: commit.subject,
        oldPath: file.old_path,
        commitFiles: allFiles,
      };
      setTabs((prev) => {
        const existing = prev.find((t) => t.id === id);
        if (existing) return prev.map((t) => (t.id === id ? tab : t));
        return [...prev, tab];
      });
      setActiveTabId(id);
    },
    []
  );

  const openGraphTab = useCallback((selectedHash?: string) => {
    const id = 'git-graph';
    if (selectedHash) setGraphSelectedHash(selectedHash);
    setTabs((prev) => {
      if (prev.some((t) => t.id === id)) return prev;
      return [...prev, { id, path: 'Git Graph', mode: 'graph' }];
    });
    setActiveTabId(id);
  }, []);

  const closeTab = (id: string) => {
    contentCacheRef.current.delete(id);
    setTabs((prev) => {
      const next = prev.filter((t) => t.id !== id);
      if (activeTabId === id) {
        setActiveTabId(next.length > 0 ? next[next.length - 1].id : null);
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
      setActiveTabId(null);
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
      setActiveTabId(null);
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

  // Measure the workbench itself (not the window): correct under CSS zoom, devtools docking, etc.
  useEffect(() => {
    const el = workbenchRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const r = entries[0].contentRect;
      setWorkbenchSize((prev) =>
        Math.round(prev.w) === Math.round(r.width) && Math.round(prev.h) === Math.round(r.height)
          ? prev
          : { w: r.width, h: r.height }
      );
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Persist preferred sizes
  useEffect(() => {
    saveLayout({ sidebarWidth, agentWidth, terminalHeight });
  }, [sidebarWidth, agentWidth, terminalHeight]);

  // The panel the user most recently opened wins when both side panels cannot fit.
  const prevShown = useRef({ sidebar: showSidebar, agent: showAgent });
  useEffect(() => {
    const prev = prevShown.current;
    if (showSidebar && !prev.sidebar) setPanelPriority('sidebar');
    else if (showAgent && !prev.agent) setPanelPriority('agent');
    prevShown.current = { sidebar: showSidebar, agent: showAgent };
  }, [showSidebar, showAgent]);

  const widths = solveWidths({
    total: workbenchSize.w,
    wantSidebar: showSidebar,
    wantAgent: showAgent,
    sidebar: sidebarWidth,
    agent: agentWidth,
    priority: panelPriority,
  });
  const termLayout = solveTerminalHeight({
    totalHeight: workbenchSize.h,
    hasTabBar: tabs.length > 0,
    preferred: terminalHeight,
  });
  // Maximized dock: tall enough for full-screen TUIs while keeping the editor/diff visible.
  const termHeight = terminalMaximized
    ? Math.max(termLayout.height, Math.min(termLayout.max, Math.round(workbenchSize.h * 0.75)))
    : termLayout.height;

  // Once shown, the terminal and agent panels stay mounted (hidden via CSS) so hiding
  // them never kills running shells or CLI agent sessions.
  const terminalMountedRef = useRef(false);
  if (showTerminal) terminalMountedRef.current = true;
  const agentMountedRef = useRef(false);
  if (widths.agentVisible) agentMountedRef.current = true;

  const toggleSidebar = () => {
    if (showSidebar && !widths.sidebarVisible) setPanelPriority('sidebar');
    else setShowSidebar((v) => !v);
  };
  const toggleAgent = () => {
    if (showAgent && !widths.agentVisible) setPanelPriority('agent');
    else setShowAgent((v) => !v);
  };

  const activeFileStatus =
    activeTab && (activeTab.mode === 'diff' || activeTab.mode === 'code')
      ? gitStatus?.files.find((f) => f.path === activeTab.path)
      : undefined;

  const activeCommitContext =
    activeTab &&
    (activeTab.mode === 'commit-diff' || activeTab.mode === 'compare-diff') &&
    activeTab.commitHash
      ? {
          mode: activeTab.mode === 'commit-diff' ? ('commit' as const) : ('compare' as const),
          hash: activeTab.commitHash,
          shortHash: activeTab.commitShortHash || activeTab.commitHash.slice(0, 7),
          subject: activeTab.commitSubject || '',
          author: activeTab.commitAuthor,
          date: activeTab.commitDate,
        }
      : null;

  const activeFileNav = (() => {
    if (!activeTab) return null;
    if (activeTab.mode === 'diff' && gitStatus && gitStatus.files.length > 1) {
      const idx = gitStatus.files.findIndex((f) => f.path === activeTab.path);
      if (idx === -1) return null;
      const go = (delta: number) => {
        const nextIdx = (idx + delta + gitStatus.files.length) % gitStatus.files.length;
        const f = gitStatus.files[nextIdx];
        openDiff(f.path, f.staged && !f.unstaged);
      };
      return {
        index: idx,
        total: gitStatus.files.length,
        onPrev: () => go(-1),
        onNext: () => go(1),
      };
    }
    if (
      (activeTab.mode === 'commit-diff' || activeTab.mode === 'compare-diff') &&
      activeTab.commitFiles &&
      activeTab.commitFiles.length > 1 &&
      activeTab.commitHash
    ) {
      const files = activeTab.commitFiles;
      const idx = files.findIndex((f) => f.path === activeTab.path);
      if (idx === -1) return null;
      const go = (delta: number) => {
        const nextIdx = (idx + delta + files.length) % files.length;
        const nextFile = files[nextIdx];
        if (activeTab.mode === 'commit-diff') {
          openCommitDiff(
            {
              hash: activeTab.commitHash!,
              short_hash: activeTab.commitShortHash || activeTab.commitHash!.slice(0, 7),
              subject: activeTab.commitSubject || '',
              author: activeTab.commitAuthor || '',
              date: activeTab.commitDate || '',
            },
            nextFile,
            files
          );
        } else {
          openCompareDiff(
            {
              hash: activeTab.commitHash!,
              short_hash: activeTab.commitShortHash || activeTab.commitHash!.slice(0, 7),
              subject: activeTab.commitSubject || '',
            },
            nextFile,
            files
          );
        }
      };
      return {
        index: idx,
        total: files.length,
        onPrev: () => go(-1),
        onNext: () => go(1),
      };
    }
    return null;
  })();

  return (
    <div className="h-screen w-screen flex flex-col overflow-hidden bg-[var(--bg-base)] text-[var(--text-primary)]">
      {/* Title Bar (custom, frameless window) */}
      <header
        onMouseDown={handleTitleBarMouseDown}
        className="h-[38px] grid grid-cols-[1fr_auto_1fr] items-center gap-3 border-b border-[var(--border)] bg-[var(--bg-activity)] shrink-0 select-none"
      >
        {/* Left: brand and workspace */}
        <div className="flex items-center gap-1 min-w-0 pl-3">
          <VireoLogo size={18} className="shrink-0 mr-1.5" />

          {editingWs ? (
            <form onSubmit={handleSwitchWorkspace} className="flex items-center min-w-0">
              <input
                value={wsDraft}
                onChange={(e) => setWsDraft(e.target.value)}
                onBlur={() => setEditingWs(false)}
                autoFocus
                className="w-72 h-7 px-2 text-[12.5px] font-mono rounded-md bg-[var(--bg-input)] border border-[var(--accent)] text-[var(--text-primary)] outline-none"
                placeholder="C:\path\to\repo"
              />
            </form>
          ) : (
            <>
              <button
                onClick={() => {
                  setWsDraft(workspace.path);
                  setEditingWs(true);
                }}
                className="flex items-center gap-1.5 h-7 px-2 rounded-md hover:bg-[var(--bg-hover)] text-[13px] font-medium text-[var(--text-primary)] max-w-[240px] cursor-pointer transition-colors"
                title={`${workspace.path}\nClick to edit the path`}
              >
                <FolderGit2 size={14} className="text-[var(--text-muted)] shrink-0" />
                <span className="truncate">{workspace.name}</span>
              </button>
              <button
                onClick={handlePickWorkspace}
                className="w-7 h-7 flex items-center justify-center rounded-md text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] shrink-0 cursor-pointer transition-colors"
                title="Open Folder"
                aria-label="Open Folder"
              >
                <FolderOpen size={15} />
              </button>
            </>
          )}
        </div>

        {/* Center: Quick Open */}
        <button
          onClick={() => setQuickOpenVisible(true)}
          className="hidden md:flex items-center gap-2 h-[26px] px-2.5 w-[300px] lg:w-[380px] rounded-md border border-[var(--border)] bg-[var(--bg-input)] hover:border-[var(--text-dim)] text-[12.5px] text-[var(--text-dim)] cursor-pointer transition-colors"
        >
          <Search size={13} className="shrink-0" />
          <span className="flex-1 text-left truncate">Search files, themes, and tasks</span>
          <kbd className="text-[10.5px] font-mono text-[var(--text-dim)]">Ctrl+P</kbd>
        </button>

        {/* Right: actions, layout toggles, window controls */}
        <div className="flex items-center justify-end h-full min-w-0">
          <div className="flex items-center gap-0.5 pr-2">
            {/* Run Scripts */}
            <div className="relative" data-no-drag>
              <button
                onClick={() => {
                  setShowRunMenu((v) => !v);
                  setShowThemeMenu(false);
                }}
                className={`h-7 px-2 text-[12.5px] rounded-md flex items-center gap-1.5 cursor-pointer transition-colors ${
                  showRunMenu
                    ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                    : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
                }`}
                title="Run a project task"
              >
                <Play size={13} className="text-[var(--added-fg)]" />
                <span className="hidden lg:inline">Run</span>
              </button>
              {showRunMenu && (
                <div className="absolute right-0 top-full mt-1.5 w-68 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-xl z-40 py-1.5">
                  <div className="px-3 py-1 text-[11px] uppercase font-semibold tracking-wider text-[var(--text-dim)]">
                    Project Tasks
                  </div>
                  {tasks.length === 0 ? (
                    <div className="px-3 py-2 text-[13px] text-[var(--text-dim)]">
                      No tasks found in package.json or Cargo.toml.
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

            {/* Theme Picker */}
            <div className="relative" data-no-drag>
              <button
                onClick={() => {
                  setShowThemeMenu((v) => !v);
                  setShowRunMenu(false);
                }}
                className={`w-7 h-7 flex items-center justify-center rounded-md cursor-pointer transition-colors ${
                  showThemeMenu
                    ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                    : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
                }`}
                title={`Theme: ${activeTheme.name}`}
                aria-label="Change theme"
              >
                <Palette size={15} />
              </button>
              {showThemeMenu && (
                <div className="absolute right-0 top-full mt-1.5 w-60 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-xl z-40 py-1.5">
                  <div className="px-3 py-1 text-[11px] uppercase font-semibold tracking-wider text-[var(--text-dim)]">
                    Color Theme
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
                      className="w-full py-1.5 px-2 text-[12.5px] text-left whitespace-nowrap rounded hover:bg-[var(--bg-hover)] text-[var(--accent)] flex items-center gap-1.5 cursor-pointer"
                      title="Import a VS Code color theme (.json)"
                    >
                      <Upload size={13} />
                      <span className="truncate">Import VS Code theme…</span>
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

            <div className="h-4 w-px bg-[var(--border)] mx-1.5" />

            {/* Layout Toggles */}
            {(
              [
                { on: widths.sidebarVisible, toggle: toggleSidebar, Icon: PanelLeft, label: 'Toggle Sidebar (Ctrl+B)' },
                { on: showTerminal, toggle: () => setShowTerminal((v) => !v), Icon: PanelBottom, label: 'Toggle Terminal (Ctrl+`)' },
                { on: widths.agentVisible, toggle: toggleAgent, Icon: PanelRight, label: 'Toggle Agent Panel' },
              ] as const
            ).map(({ on, toggle, Icon, label }) => (
              <button
                key={label}
                onClick={toggle}
                className={`w-7 h-7 flex items-center justify-center rounded-md cursor-pointer transition-colors ${
                  on
                    ? 'text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
                    : 'text-[var(--text-dim)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-muted)]'
                }`}
                title={label}
                aria-label={label}
                aria-pressed={on}
              >
                <Icon size={15} />
              </button>
            ))}
          </div>

          {/* Window Controls */}
          {nativeWindow && (
            <div className="flex h-full shrink-0">
              <button
                onClick={() => windowAction('minimize')}
                className="w-[46px] h-full flex items-center justify-center text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] transition-colors"
                title="Minimize"
                aria-label="Minimize"
              >
                <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
                  <path d="M0 5.5h10" stroke="currentColor" strokeWidth="1" />
                </svg>
              </button>
              <button
                onClick={() => windowAction('toggle_maximize')}
                className="w-[46px] h-full flex items-center justify-center text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] transition-colors"
                title={nativeWindow.maximized ? 'Restore' : 'Maximize'}
                aria-label={nativeWindow.maximized ? 'Restore' : 'Maximize'}
              >
                {nativeWindow.maximized ? (
                  <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                    <path d="M2.5 2.5V0.5h7v7h-2" stroke="currentColor" strokeWidth="1" />
                    <rect x="0.5" y="2.5" width="7" height="7" stroke="currentColor" strokeWidth="1" />
                  </svg>
                ) : (
                  <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                    <rect x="0.5" y="0.5" width="9" height="9" stroke="currentColor" strokeWidth="1" />
                  </svg>
                )}
              </button>
              <button
                onClick={() => windowAction('close')}
                className="w-[46px] h-full flex items-center justify-center text-[var(--text-muted)] hover:bg-[#e81123] hover:text-white transition-colors"
                title="Close"
                aria-label="Close"
              >
                <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
                  <path d="M0.5 0.5l9 9M9.5 0.5l-9 9" stroke="currentColor" strokeWidth="1" />
                </svg>
              </button>
            </div>
          )}
        </div>
      </header>

      {/* Main Workbench Area */}
      <div ref={workbenchRef} className="flex-1 flex min-h-0 min-w-0 overflow-hidden">
        {/* Left Sidebar */}
        {widths.sidebarVisible && (
          <>
            <div style={{ width: widths.sidebarWidth }} className="shrink-0 h-full overflow-hidden">
              <Sidebar
                activeTab={sidebarTab}
                onTabChange={(t) => {
                  if (t === sidebarTab) setShowSidebar(false);
                  else setSidebarTab(t);
                }}
                gitStatus={gitStatus}
                selectedPath={activeTab?.mode === 'diff' || activeTab?.mode === 'code' ? activeTab.path : null}
                selectedTabId={activeTabId}
                workspacePath={workspace.path}
                onOpenDiff={openDiff}
                onOpenFile={openFile}
                onOpenCommitDiff={openCommitDiff}
                onOpenCompareDiff={openCompareDiff}
                onOpenGraphTab={openGraphTab}
                onRefreshGit={refreshGit}
                onToast={showToast}
                fsVersion={fsVersion}
                recentPaths={tabPulse}
                onAskAgent={handleAskAgent}
              />
            </div>
            <ResizeHandle
              orientation="vertical"
              direction={1}
              label="Resize sidebar"
              value={widths.sidebarWidth}
              min={SIDEBAR.min}
              max={widths.sidebarMax}
              onChange={setSidebarWidth}
              onReset={() => setSidebarWidth(SIDEBAR.def)}
            />
          </>
        )}

        {/* Collapsed sidebar: keep the icon rail so the panel can be reopened from it */}
        {!widths.sidebarVisible && (
          <div className="shrink-0 h-full overflow-hidden">
            <Sidebar
              railOnly
              activeTab={sidebarTab}
              onTabChange={(t) => {
                setSidebarTab(t);
                if (showSidebar) setPanelPriority('sidebar');
                else setShowSidebar(true);
              }}
              gitStatus={gitStatus}
              selectedPath={null}
              selectedTabId={null}
              workspacePath={workspace.path}
              onOpenDiff={openDiff}
              onOpenFile={openFile}
              onOpenCommitDiff={openCommitDiff}
              onOpenCompareDiff={openCompareDiff}
              onOpenGraphTab={openGraphTab}
              onRefreshGit={refreshGit}
              onToast={showToast}
              fsVersion={fsVersion}
              onAskAgent={handleAskAgent}
            />
          </div>
        )}

        {/* Center Column: Diff/Code/Graph + Bottom Terminal */}
        <div className="flex-1 flex flex-col min-w-0 min-h-0">
          {/* Open Tabs Bar */}
          {tabs.length > 0 && (
            <div
              className="flex items-center h-10 bg-[var(--bg-sidebar)] border-b border-[var(--border)] overflow-x-auto shrink-0"
              onWheelCapture={() => (userActivityRef.current = Date.now())}
              onPointerDownCapture={() => (userActivityRef.current = Date.now())}
            >
              {tabs.map((tab) => {
                const active = tab.id === activeTabId;
                const baseName = tab.path.split('/').pop() || tab.path;
                const label =
                  tab.mode === 'graph'
                    ? 'Git Graph'
                    : tab.mode === 'commit-diff'
                    ? `${baseName} (${tab.commitShortHash})`
                    : tab.mode === 'compare-diff'
                    ? `${baseName} (${tab.commitShortHash}↔WT)`
                    : baseName;
                return (
                  <div
                    key={tab.id}
                    onClick={() => setActiveTabId(tab.id)}
                    className={`group flex items-center gap-2 px-3.5 h-full text-[13px] border-r border-[var(--border)] cursor-pointer select-none shrink-0 transition-colors ${
                      tabPulse[tab.path] !== undefined
                        ? `tab-pulse-${tabPulse[tab.path] % 2}`
                        : ''
                    } ${
                      active
                        ? 'bg-[var(--bg-base)] text-[var(--text-primary)] font-medium border-t-2 border-t-[var(--accent)]'
                        : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                    }`}
                  >
                    {tab.mode === 'graph' ? (
                      <GitFork size={14} className="text-[var(--accent)] shrink-0" />
                    ) : tab.mode === 'commit-diff' ? (
                      <GitCommitHorizontal size={14} className="text-[var(--accent)] shrink-0" />
                    ) : tab.mode === 'diff' || tab.mode === 'compare-diff' ? (
                      <GitCompare size={14} className="text-[var(--modified-fg)] shrink-0" />
                    ) : (
                      <FileIcon path={tab.path} size={14} />
                    )}
                    <span>{label}</span>
                    {(tab.mode === 'diff' || tab.mode === 'code') &&
                      (() => {
                        const edit = liveEdits.find((e) => e.path === tab.path);
                        return edit ? (
                          <span
                            className="font-mono text-[10px] tabular-nums"
                            title="Changed by an agent this session"
                          >
                            <span className="text-[var(--added-fg)]">+{edit.adds}</span>{' '}
                            <span className="text-[var(--removed-fg)]">-{edit.dels}</span>
                          </span>
                        ) : null;
                      })()}
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        closeTab(tab.id);
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
          <div
            className="flex-1 flex flex-col min-h-0 relative"
            onWheelCapture={() => (userActivityRef.current = Date.now())}
            onKeyDownCapture={() => (userActivityRef.current = Date.now())}
            onPointerDownCapture={() => (userActivityRef.current = Date.now())}
          >
            {!activeTab ? (
              <div className="flex-1 min-h-0 overflow-y-auto flex flex-col items-center justify-center-safe p-8 text-center gap-5">
                <VireoLogo size={56} className="drop-shadow-lg" />
                <div className="space-y-1.5 max-w-md">
                  <h2 className="text-lg font-semibold tracking-tight text-[var(--text-primary)]">
                    Welcome to Vireo
                  </h2>
                  <p className="text-[13.5px] text-[var(--text-muted)] leading-relaxed">
                    Review your coding agent's changes as they happen, inspect Git history & branches,
                    and commit to GitHub, all from one lightweight window.
                  </p>
                </div>
                <div className="flex flex-wrap items-center justify-center gap-2.5 text-[13px]">
                  <button
                    onClick={() => setQuickOpenVisible(true)}
                    className="px-3.5 py-2 rounded-lg bg-[var(--accent)] text-[var(--accent-fg)] font-medium cursor-pointer hover:opacity-95 transition-opacity"
                  >
                    Quick Open (Ctrl+P)
                  </button>
                  {gitStatus?.is_repo && (
                    <button
                      onClick={() => openGraphTab()}
                      className="px-3.5 py-2 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] flex items-center gap-1.5 cursor-pointer transition-colors"
                    >
                      <GitFork size={15} className="text-[var(--accent)]" />
                      Git Commit Graph
                    </button>
                  )}
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
            ) : activeTab.mode === 'graph' ? (
              <Suspense
                fallback={
                  <div className="flex-1 flex items-center justify-center text-xs text-[var(--text-dim)]">
                    Loading Git Graph...
                  </div>
                }
              >
                <CommitGraphView
                  gitStatus={gitStatus}
                  workspacePath={workspace.path}
                  initialSelectedHash={graphSelectedHash}
                  onOpenCommitDiff={openCommitDiff}
                  onOpenCompareDiff={openCompareDiff}
                  onOpenDiff={openDiff}
                  onRefreshGit={refreshGit}
                  onToast={showToast}
                />
              </Suspense>
            ) : activeTab.mode === 'diff' ||
              activeTab.mode === 'commit-diff' ||
              activeTab.mode === 'compare-diff' ? (
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
                  commitContext={activeCommitContext}
                  fileNav={activeFileNav}
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
                    markSelfWrite(activeTab.path);
                    await api.gitDiscard(activeTab.path, !!activeFileStatus?.untracked);
                    await refreshGit();
                    reloadActiveContent();
                    showToast(`Discarded changes in ${activeTab.path}`);
                  }}
                  onApplyHunk={async (patch, action) => {
                    try {
                      const cached = action === 'stage' || action === 'unstage';
                      const reverse = action === 'unstage' || action === 'revert';
                      if (action === 'revert') markSelfWrite(activeTab.path);
                      const res = await api.gitApplyPatch(patch, cached, reverse);
                      if (!res.ok) {
                        throw new Error(res.stderr || `Failed to ${action} hunk`);
                      }
                      await refreshGit();
                      await reloadActiveContent();
                      showToast(
                        action === 'stage'
                          ? 'Staged hunk'
                          : action === 'unstage'
                          ? 'Unstaged hunk'
                          : 'Reverted hunk'
                      );
                    } catch (e: unknown) {
                      showToast(e instanceof Error ? e.message : `Failed to ${action} hunk`, true);
                    }
                  }}
                  onSwitchToCode={() => openFile(activeTab.path)}
                  onAskAgent={handleAskAgent}
                  onToast={showToast}
                  liveFlash={
                    liveFlash &&
                    activeTab.mode === 'diff' &&
                    !activeTab.staged &&
                    diffData?.path === liveFlash.path
                      ? { key: liveFlash.key, ranges: liveFlash.ranges }
                      : null
                  }
                  followEdits={followEdits}
                  userActivityRef={userActivityRef}
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
                  content={fileData && fileData.path === activeTab.path ? fileData.content : ''}
                  binary={fileData && fileData.path === activeTab.path ? fileData.binary : false}
                  followEdits={followEdits}
                  userActivityRef={userActivityRef}
                  onDirtyChange={(d) => {
                    editorDirtyRef.current = d;
                  }}
                  reviewBase={review[activeTab.path] ?? null}
                  onReviewBaseChange={(b) => setReviewBaseFor(activeTab.path, b)}
                  onFileUndone={(before, base) => handleFileUndone(activeTab.path, before, base)}
                  scrollTarget={
                    liveFlash && liveFlash.path === activeTab.path && liveFlash.ranges.length > 0
                      ? { key: liveFlash.key, line: liveFlash.ranges[0].from }
                      : null
                  }
                  theme={activeTheme}
                  hasDiff={!!activeFileStatus}
                  jumpToLine={activeTab.line}
                  jumpToColumn={activeTab.col}
                  jumpToEndColumn={activeTab.colEnd}
                  jumpKey={activeTab.jumpKey}
                  onSave={async (newContent) => {
                    markSelfWrite(activeTab.path);
                    await api.writeFile(activeTab.path, newContent);
                    snapshotsRef.current.set(activeTab.path, newContent);
                    setFileData({ path: activeTab.path, content: newContent, binary: false });
                    await refreshGit();
                    showToast(`Saved ${activeTab.path}`);
                  }}
                  onSwitchToDiff={() => openDiff(activeTab.path)}
                  onAskAgent={handleAskAgent}
                />
              </Suspense>
            )}

            {/* Agent edits bar: docked under the viewer (never covers code), same chrome as the toolbars */}
          </div>

          {!pillDismissed &&
            (pillVisible || pausedJump || reviewPaths.length > 0) &&
            liveEdits.length > 0 && (
              <div className="shrink-0 flex items-center gap-2 h-9 px-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)] text-xs">
                <AgentEditsStrip
                  edits={liveEdits}
                  activePath={activeTab?.path ?? null}
                  tabPulse={tabPulse}
                  onFocus={focusPath}
                />
                <div className="flex items-center gap-1.5 shrink-0">
                  {pausedJump && (
                    <button
                      onClick={() => focusPath(pausedJump)}
                      className="h-6 px-2 rounded-md bg-[var(--accent)] text-[var(--accent-fg)] font-medium flex items-center gap-1.5 cursor-pointer hover:opacity-90"
                    >
                      Show latest
                    </button>
                  )}
                  {reviewPaths.length > 0 && (
                    <>
                      <button
                        onClick={() =>
                          openDiff(
                            activeTab && reviewPaths.includes(activeTab.path)
                              ? activeTab.path
                              : reviewPaths[reviewPaths.length - 1]
                          )
                        }
                        className="h-6 px-2 rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] flex items-center gap-1.5 cursor-pointer transition-colors"
                        title="Open the side-by-side diff"
                      >
                        <GitCompare size={12} />
                        Diff
                      </button>
                      <button
                        onClick={() => {
                          if (!undoConfirm) {
                            setUndoConfirm(true);
                            window.setTimeout(() => setUndoConfirm(false), 3000);
                          } else {
                            setUndoConfirm(false);
                            void undoAll();
                          }
                        }}
                        className={`h-6 px-2 rounded-md border flex items-center gap-1.5 cursor-pointer transition-colors ${
                          undoConfirm
                            ? 'border-[var(--removed-fg)] bg-[var(--removed-bg)] text-[var(--removed-fg)]'
                            : 'border-[var(--border)] bg-[var(--bg-elevated)] text-[var(--removed-fg)] hover:bg-[var(--removed-bg)]'
                        }`}
                        title={`Revert ${Object.keys(review).length} file${Object.keys(review).length === 1 ? '' : 's'} to before the agent's edits (you can redo):\n${Object.keys(review).join('\n')}`}
                      >
                        <RotateCcw size={12} />
                        {undoConfirm
                          ? `Confirm undo (${Object.keys(review).length})`
                          : 'Undo all'}
                      </button>
                      <button
                        onClick={keepAll}
                        className="h-6 px-2 rounded-md bg-[var(--accent)] text-[var(--accent-fg)] font-medium flex items-center gap-1.5 cursor-pointer hover:opacity-90"
                        title="Accept all agent edits"
                      >
                        <Check size={12} />
                        Keep all
                      </button>
                    </>
                  )}
                  <button
                    onClick={toggleFollow}
                    aria-pressed={followEdits}
                    className={`h-6 w-6 rounded-md border flex items-center justify-center cursor-pointer transition-colors ${
                      followEdits
                        ? 'border-[var(--border)] bg-[var(--bg-active)] text-[var(--accent)]'
                        : 'border-[var(--border)] text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                    }`}
                    title={
                      followEdits
                        ? 'Following edits (pauses while you type or scroll). Click to turn off.'
                        : 'Follow edits is off. Click to turn on.'
                    }
                  >
                    <Crosshair size={13} />
                  </button>
                  <button
                    onClick={() => {
                      setPillDismissed(true);
                      setPausedJump(null);
                    }}
                    className="h-6 w-6 rounded-md flex items-center justify-center text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
                    title="Hide"
                  >
                    <X size={13} />
                  </button>
                </div>
              </div>
            )}

          {/* Bottom Terminal */}
          {(showTerminal || terminalMountedRef.current) && (
            <div className={showTerminal ? 'contents' : 'hidden'}>
              <ResizeHandle
                orientation="horizontal"
                direction={-1}
                label="Resize terminal"
                value={termLayout.height}
                min={TERMINAL.min}
                max={termLayout.max}
                onChange={setTerminalHeight}
                onReset={() => setTerminalHeight(TERMINAL.def)}
              />
              <div style={{ height: termHeight }} className="shrink-0 min-h-0 overflow-hidden">
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
                    launcherSignal={launcherSignal}
                    maximized={terminalMaximized}
                    onToggleMaximize={() => setTerminalMaximized((v) => !v)}
                    onSendOutputToAgent={handleAskAgent}
                    onClosePanel={() => setShowTerminal(false)}
                  />
                </Suspense>
              </div>
            </div>
          )}
        </div>

        {/* Right Agent Panel (stays mounted while hidden so CLI agents and chats survive) */}
        {(widths.agentVisible || agentMountedRef.current) && (
          <div className={widths.agentVisible ? 'contents' : 'hidden'}>
            <ResizeHandle
              orientation="vertical"
              direction={-1}
              label="Resize agent panel"
              value={widths.agentWidth}
              min={AGENT.min}
              max={widths.agentMax}
              onChange={setAgentWidth}
              onReset={() => setAgentWidth(AGENT.def)}
            />
            <div style={{ width: widths.agentWidth }} className="shrink-0 h-full overflow-hidden">
              <AgentPanel
                theme={activeTheme}
                workspacePath={workspace.path}
                draftAppend={agentDraftAppend}
                activeFilePath={
                  activeTab && (activeTab.mode === 'code' || activeTab.mode === 'diff')
                    ? activeTab.path
                    : null
                }
                changedFileCount={gitStatus?.files.length ?? 0}
                changedFiles={gitStatus?.files ?? []}
                branch={gitStatus?.branch ?? ''}
                allFiles={allFiles}
                onLaunchCliAgent={() => {
                  setShowTerminal(true);
                  setLauncherSignal((n) => n + 1);
                }}
                onOpenDiff={(p) => openDiff(p)}
                onRefreshGit={refreshGit}
                onToast={showToast}
              />
            </div>
          </div>
        )}
      </div>

      {/* Status Bar */}
      <footer
        className="h-6 flex items-stretch justify-between text-[12px] leading-none border-t border-[var(--border)] shrink-0 select-none"
        style={{
          background: 'var(--status-bar-bg)',
          color: 'var(--status-bar-fg)',
        }}
      >
        <div className="flex items-stretch min-w-0 pl-1">
          <button
            onClick={() => {
              setShowSidebar(true);
              setSidebarTab('changes');
            }}
            className="flex items-center gap-1.5 px-2 status-item"
            title={gitStatus?.is_repo ? 'Show changes' : 'This folder is not a Git repository'}
          >
            <GitBranch size={12} className="shrink-0" />
            <span className="truncate max-w-[180px]">
              {gitStatus?.is_repo ? gitStatus.branch || 'HEAD' : 'No Git'}
            </span>
          </button>
          {gitStatus?.is_repo && (
            <button
              onClick={() => {
                setShowSidebar(true);
                setSidebarTab('changes');
              }}
              className="flex items-center gap-2 px-2 tabular-nums status-item"
              title={`${gitStatus.files.length} changed ${gitStatus.files.length === 1 ? 'file' : 'files'}`}
            >
              <span className="flex items-center gap-1">
                <FileCode size={12} className="shrink-0" />
                {gitStatus.files.length}
              </span>
              {(gitStatus.total_additions > 0 || gitStatus.total_deletions > 0) && (
                <span className="flex items-center gap-1.5">
                  <span className="text-[var(--added-fg)]">+{gitStatus.total_additions}</span>
                  <span className="text-[var(--danger)]">-{gitStatus.total_deletions}</span>
                </span>
              )}
            </button>
          )}
          <span
            className="flex items-center gap-1.5 px-2 opacity-80"
            title={liveConnected ? 'Watching the workspace for changes' : 'Reconnecting to the file watcher'}
          >
            <span
              className="w-1.5 h-1.5 rounded-full"
              style={{ background: liveConnected ? '#10b981' : '#f59e0b' }}
            />
            {liveConnected ? 'Live' : 'Reconnecting'}
          </span>
        </div>

        <div className="flex items-stretch pr-1">
          <div className="group flex items-stretch" title="Zoom (Ctrl+= / Ctrl+- / Ctrl+0)">
            <button
              onClick={() => adjustZoom((z) => z - ZOOM_STEP)}
              className="px-1.5 flex items-center opacity-0 group-hover:opacity-80 hover:!opacity-100 status-item"
              aria-label="Zoom out"
            >
              <Minus size={11} />
            </button>
            <button
              onClick={() => adjustZoom(1.0)}
              className="px-1 flex items-center tabular-nums status-item"
              title="Reset zoom to 100%"
            >
              {Math.round(zoom * 100)}%
            </button>
            <button
              onClick={() => adjustZoom((z) => z + ZOOM_STEP)}
              className="px-1.5 flex items-center opacity-0 group-hover:opacity-80 hover:!opacity-100 status-item"
              aria-label="Zoom in"
            >
              <Plus size={11} />
            </button>
          </div>
          <button
            onClick={() => {
              setShowThemeMenu((v) => !v);
              setShowRunMenu(false);
            }}
            className="flex items-center gap-1.5 px-2 status-item"
            title="Change theme"
          >
            <Palette size={12} className="shrink-0" />
            {activeTheme.name}
          </button>
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
          {toast.action && (
            <button
              onClick={() => {
                const run = toast.action!.run;
                setToast(null);
                run();
              }}
              className="ml-3 underline underline-offset-2 cursor-pointer"
            >
              {toast.action.label}
            </button>
          )}
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
