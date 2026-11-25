import React, {
  Suspense,
  lazy,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
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
  Keyboard,
  Rocket,
  Minus,
  Palette,
  PanelBottom,
  PanelLeft,
  PanelRight,
  Play,
  RotateCcw,
  Plus,
  Search,
  Settings,
  Upload,
  X,
  FileCode,
} from 'lucide-react';
import { diffLines } from 'diff';
import { AgentSettingsProvider } from './agentSettings';
import { findAction, shortcutLabel, withShortcut, useKeybindings, type ActionId } from './keybindings';
import { api, wsUrl } from './api';
import { gitWriteActive } from './utils/gitWrites';
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
import { RunMenu, type TaskDraft } from './components/RunMenu';
import type { PendingTaskCommand, TaskLaunch } from './components/TerminalPanel';
import {
  activeFileTask,
  expandVars,
  rankTasks,
  slugify,
  useTaskPrefs,
} from './utils/tasks';
import { Sidebar, SidebarTab } from './components/Sidebar';
import { AgentEditsStrip } from './components/AgentEditsStrip';
import { FileIcon } from './components/FileIcon';
import { FloatingContextMenu, type ContextMenuItem } from './components/CommitContextMenu';
import { VireoLogo } from './components/VireoLogo';
import Toast, { type ToastData } from './components/Toast';
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
import { flushPrefs, hasPersistedPref, setPref, setPrefs } from './prefs';

/** Files whose changes can add, remove or rename Run tasks. */
const TASK_SOURCE_RE =
  /(^|\/)(package\.json|Cargo\.toml|pyproject\.toml|go\.mod|[Mm]akefile|justfile|Taskfile\.ya?ml|(docker-)?compose\.ya?ml|deno\.jsonc?|tasks\.json|[^/]+\.(ps1|sh|bat|cmd))$/;

const DiffViewer = lazy(() =>
  import('./components/DiffViewer').then((m) => ({ default: m.DiffViewer }))
);
const SettingsPage = lazy(() =>
  import('./components/SettingsPage').then((m) => ({ default: m.SettingsPage }))
);
const ShortcutsPage = lazy(() =>
  import('./components/ShortcutsPage').then((m) => ({ default: m.ShortcutsPage }))
);
const ReleasePage = lazy(() =>
  import('./components/release/ReleasePage').then((m) => ({ default: m.ReleasePage }))
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
const AgentPanel = lazy(() =>
  import('./components/AgentPanel').then((m) => ({ default: m.AgentPanel }))
);
const QuickOpen = lazy(() =>
  import('./components/QuickOpen').then((m) => ({ default: m.QuickOpen }))
);

/** Cap on cached file/diff text held in JS memory (chars, roughly bytes for source code). */
const CACHE_BYTE_BUDGET = 12 * 1024 * 1024;

/** Evict least-recently-inserted entries until the total size fits the budget (keeps the newest). */
function trimToBudget<V>(map: Map<string, V>, sizeOf: (v: V) => number, budget = CACHE_BYTE_BUDGET) {
  let total = 0;
  for (const v of map.values()) total += sizeOf(v);
  while (total > budget && map.size > 1) {
    const oldest = map.keys().next().value as string;
    total -= sizeOf(map.get(oldest) as V);
    map.delete(oldest);
  }
}

const ZOOM_MIN = 0.7;
const ZOOM_MAX = 2.0;
const ZOOM_STEP = 0.1;
const REPEATABLE = new Set<ActionId>(['zoomIn', 'zoomOut', 'nextTab', 'prevTab']);
/** Minimum time between automatic follow-edits tab switches. */
const FOLLOW_SWITCH_DWELL_MS = 700;
/** Dirty files whose current text is captured up front as their pre-agent snapshot. */
const SEED_SNAPSHOT_LIMIT = 40;

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

  const themeMetaJson = (theme: LiteTheme) =>
    JSON.stringify({ type: theme.type, vars: theme.vars });

  const selectTheme = useCallback(
    (target: string | LiteTheme) => {
      const next =
        typeof target === 'string'
          ? themes.find((t) => t.id === target) || BUILTIN_THEMES[0]
          : target;
      setThemeId(next.id);
      setPrefs({
        'vireo-theme': next.id,
        'vireo-theme-meta': themeMetaJson(next),
      });
    },
    [themes]
  );

  useLayoutEffect(() => {
    applyTheme(activeTheme);
  }, [activeTheme]);

  // One-time backfill for browser/dev sessions that saved a theme only in localStorage.
  useEffect(() => {
    try {
      const storedTheme = localStorage.getItem('vireo-theme');
      if (storedTheme && !hasPersistedPref('vireo-theme')) {
        const entry: Record<string, string> = {
          'vireo-theme': activeTheme.id,
          'vireo-theme-meta': themeMetaJson(activeTheme),
        };
        const storedCustom = localStorage.getItem('vireo-custom-themes');
        if (storedCustom && !hasPersistedPref('vireo-custom-themes')) {
          entry['vireo-custom-themes'] = storedCustom;
        }
        setPrefs(entry);
      }
    } catch {
      // ignore
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Workspace & Git state
  const [workspace, setWorkspace] = useState<WorkspaceInfo>({ path: '', name: 'Vireo' });
  const [editingWs, setEditingWs] = useState(false);
  const [wsDraft, setWsDraft] = useState('');
  const [gitStatus, setGitStatus] = useState<GitStatusRes | null>(null);
  const [fsVersion, setFsVersion] = useState(0);
  const [allFiles, setAllFiles] = useState<string[]>([]);
  const [tasks, setTasks] = useState<TaskItem[]>([]);
  const [projectDefaultTask, setProjectDefaultTask] = useState<string | null>(null);
  const [runningTaskIds, setRunningTaskIds] = useState<string[]>([]);
  const { prefs: taskPrefs, setPinned, recordRun, saveLocal, removeLocal } = useTaskPrefs(
    workspace.path
  );
  const [liveConnected, setLiveConnected] = useState(false);

  // Native window state for the custom title bar (null = running in a browser)
  const [nativeWindow, setNativeWindow] = useState<{ maximized: boolean } | null>(null);
  const windowAction = useCallback(
    (action: 'state' | 'minimize' | 'toggle_maximize' | 'close' | 'drag') => {
      if (action === 'close') flushPrefs();
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
  const [showAgent, setShowAgent] = useState(false);
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
  const [toast, setToast] = useState<ToastData | null>(null);

  // Tabs & Active content
  const [tabs, setTabs] = useState<OpenTab[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);
  const [tabMenu, setTabMenu] = useState<{ x: number; y: number; tabId: string } | null>(null);
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
    /** 1-based new-side lines the edit added. */
    ranges: Array<{ from: number; to: number }>;
    /** New-side line where the edit is (valid for pure deletions too). */
    anchor: number;
    /** The user asked to see it: scroll even when not following / busy. */
    reveal?: boolean;
    /** File text (LF) right after the edit: the editor waits until it shows exactly this. */
    text?: string;
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
  /** path -> where the agent's latest edit to it is, so revisiting the file lands on it. */
  const lastEditRef = useRef<Map<string, { ranges: Array<{ from: number; to: number }>; anchor: number }>>(
    new Map()
  );
  /** Follow-edits tab switching is rate limited so a multi-file burst doesn't strobe the editor. */
  const followSwitchRef = useRef<{ at: number; timer: number; path: string | null }>({
    at: 0,
    timer: 0,
    path: null,
  });
  /**
   * Files that were already dirty when first seen (path -> when their text was captured as the
   * pre-agent snapshot). Without this, the first agent edit to such a file falls back to git HEAD
   * as "before" and paints the user's own uncommitted work as agent changes.
   */
  const seededRef = useRef<Map<string, number>>(new Map());
  const pillTimerRef = useRef<number | undefined>(undefined);
  const tabsRef = useRef<OpenTab[]>([]);
  const actionsRef = useRef<Partial<Record<ActionId, () => void>>>({});
  useKeybindings(); // re-render so tooltips follow rebinding
  const gitStatusRef = useRef<GitStatusRes | null>(null);
  /**
   * `git status` requests resolve out of order. `seq` numbers requests, `applied` is the newest one
   * shown, and `startedAt` is when that one began (it reflects disk state from that moment on).
   */
  const gitFetchRef = useRef({ seq: 0, applied: 0, startedAt: 0 });
  /** When the watcher last reported each path changed (normalized); see the prune effect below. */
  const editedAtRef = useRef<Map<string, number>>(new Map());
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
  const [pendingCmd, setPendingCmd] = useState<PendingTaskCommand | null>(null);
  const [agentDraftAppend, setAgentDraftAppend] = useState<{
    id: number;
    text: string;
  } | null>(null);

  const themeFileInputRef = useRef<HTMLInputElement>(null);

  const toastSeq = useRef(0);
  const showToast = useCallback((msg: string, isError = false, action?: ToastAction) => {
    setToast({ id: ++toastSeq.current, msg, isError, action });
  }, []);

  const refreshGit = useCallback(async () => {
    const attempt = async (retries: number): Promise<void> => {
      const f = gitFetchRef.current;
      const seq = ++f.seq;
      const startedAt = Date.now();
      try {
        const status = await api.gitStatus();
        // A slower, older request must not overwrite a newer answer (it can predate an agent edit).
        if (seq < f.applied) return;
        f.applied = seq;
        f.startedAt = startedAt;
        setGitStatus(status);
      } catch {
        // A failed fetch (git busy, backend restarting) must not leave the sidebar stale until the
        // user hits refresh: retry unless a newer request has been issued meanwhile.
        if (retries > 0) {
          window.setTimeout(() => {
            if (f.seq === seq) void attempt(retries - 1);
          }, 400);
        }
      }
    };
    await attempt(2);
  }, []);

  const loadWorkspaceData = useCallback(async () => {
    try {
      const current = await api.getWorkspace();
      if (!current.path) {
        // No folder is open yet: show the empty shell and wait for the user to open one.
        setWorkspace(current);
        setWsDraft('');
        return;
      }
      const gf = gitFetchRef.current;
      const gitSeq = ++gf.seq;
      const gitStartedAt = Date.now();
      const [ws, status, files, taskList] = await Promise.all([
        Promise.resolve(current),
        api.gitStatus(),
        api.listFiles(),
        api.getTasks(),
      ]);
      setWorkspace(ws);
      setWsDraft(ws.path);
      if (gitSeq >= gf.applied) {
        gf.applied = gitSeq;
        gf.startedAt = gitStartedAt;
        setGitStatus(status);
      }
      setAllFiles(files);
      setTasks(taskList.tasks);
      setProjectDefaultTask(taskList.default_task);

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

  /** Store a tab's freshly loaded content (LRU, bounded by count and total size). */
  const rememberContent = useCallback(
    (id: string, patch: { diff?: GitDiffRes; file?: { path: string; content: string; binary: boolean } }) => {
      const cache = contentCacheRef.current;
      const prev = cache.get(id);
      cache.delete(id);
      cache.set(id, { ...prev, ...patch });
      while (cache.size > 30) cache.delete(cache.keys().next().value as string);
      trimToBudget(
        cache,
        (c) =>
          (c.diff ? c.diff.original.length + c.diff.modified.length + (c.diff.patch?.length ?? 0) : 0) +
          (c.file?.content.length ?? 0)
      );
    },
    []
  );

  const reloadActiveContent = useCallback(async () => {
    if (!activeTab) {
      setDiffData(null);
      setFileData(null);
      return;
    }
    if (
      activeTab.mode === 'graph' ||
      activeTab.mode === 'settings' ||
      activeTab.mode === 'shortcuts' ||
      activeTab.mode === 'release'
    ) {
      return;
    }
    const id = activeTab.id;
    const cache = contentCacheRef.current;
    const isCurrent = () => activeTabIdRef.current === id;
    const remember = (patch: { diff?: GitDiffRes; file?: { path: string; content: string; binary: boolean } }) =>
      rememberContent(id, patch);
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
  }, [activeTab, rememberContent]);

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

  useEffect(() => {
    reloadActiveContent();
  }, [reloadActiveContent]);

  const reloadActiveContentRef = useRef(reloadActiveContent);
  useEffect(() => {
    reloadActiveContentRef.current = reloadActiveContent;
  }, [reloadActiveContent]);

  // Apply zoom to native Tauri WebView2 (or fallback to CSS zoom in browser mode)
  const lastSavedZoomRef = useRef(zoom);
  useEffect(() => {
    let cancelled = false;
    if (lastSavedZoomRef.current !== zoom) {
      lastSavedZoomRef.current = zoom;
      setPref('vireo-zoom', String(zoom));
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

  const refreshTasks = useCallback(() => {
    api
      .getTasks()
      .then((res) => {
        setTasks(res.tasks);
        setProjectDefaultTask(res.default_task);
      })
      .catch(() => {});
  }, []);
  const refreshTasksRef = useRef(refreshTasks);
  refreshTasksRef.current = refreshTasks;

  // Live file-watcher WebSocket so diffs auto-refresh as agents code!
  useEffect(() => {
    let closed = false;
    let sock: WebSocket | null = null;
    let timer: number | undefined;

    // Bursts of file events (builds, installs, multi-file agent edits) are coalesced into one
    // refresh, and a hidden window defers it until it is visible again.
    const pendingPaths = new Set<string>();
    let coarse = false; // some batch said "something changed" without listing paths
    let dirty = false;
    let flushTimer = 0;
    let firstAt = 0;
    const flush = () => {
      flushTimer = 0;
      firstAt = 0;
      if (!dirty || closed) return;
      if (document.hidden) return; // resumed by onVisible
      dirty = false;
      const paths = coarse ? [] : Array.from(pendingPaths);
      pendingPaths.clear();
      coarse = false;
      fsChangedRef.current(paths, new Set(gitStatusRef.current?.files.map((f) => f.path) ?? []));
      if (paths.length === 0 || paths.some((p) => TASK_SOURCE_RE.test(p))) {
        refreshTasksRef.current();
      }
      setFsVersion((v) => v + 1);
      refreshGit();
      reloadActiveContentRef.current();
    };
    const scheduleFlush = () => {
      const now = Date.now();
      if (!firstAt) firstAt = now;
      window.clearTimeout(flushTimer);
      // The backend watcher already debounces; this only merges back-to-back messages (watcher
      // batch + tool notification). Trailing 40 ms, never later than 250 ms after the first.
      flushTimer = window.setTimeout(flush, Math.max(0, Math.min(40, firstAt + 250 - now)));
    };
    const onVisible = () => {
      if (!document.hidden && dirty) scheduleFlush();
    };
    document.addEventListener('visibilitychange', onVisible);
    const onFocus = () => refreshGit();
    window.addEventListener('focus', onFocus);

    const connect = () => {
      if (closed) return;
      sock = new WebSocket(wsUrl('/ws/events'));
      sock.onopen = () => {
        setLiveConnected(true);
        // Events sent while the socket was down are lost: resync.
        refreshGit();
        setFsVersion((v) => v + 1);
      };
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
            const paths: string[] = Array.isArray(data.paths) ? data.paths : [];
            if (paths.length === 0) coarse = true;
            else for (const p of paths) pendingPaths.add(p);
            dirty = true;
            scheduleFlush();
          } else if (data.type === 'workspace_switched') {
            snapshotsRef.current.clear();
            contentCacheRef.current.clear();
            setLiveEdits([]);
            setReview({});
            setPillDismissed(false);
            setTabPulse({});
            setPausedJump(null);
            setLiveFlash(null);
            lastEditRef.current.clear();
            seededRef.current.clear();
            window.clearTimeout(followSwitchRef.current.timer);
            followSwitchRef.current.path = null;
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
      window.clearTimeout(flushTimer);
      document.removeEventListener('visibilitychange', onVisible);
      window.removeEventListener('focus', onFocus);
      sock?.close();
    };
  }, [refreshGit, loadWorkspaceData]);

  // Global keyboard shortcuts (bindings come from ./keybindings; handlers are in `actionsRef`).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const def = findAction(e);
      if (!def) return;
      // Like VS Code, app shortcuts also work while the terminal has focus.
      const run = actionsRef.current[def.id];
      if (!run) return;
      e.preventDefault();
      e.stopPropagation();
      // Holding a key must not flicker toggles; zoom and tab cycling may repeat.
      if (e.repeat && !REPEATABLE.has(def.id)) return;
      run();
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

  /** Glow + scroll to the agent's latest edit in `path` (for "Show latest" / edits-bar clicks). */
  const revealEdit = useCallback((path: string) => {
    const e = lastEditRef.current.get(path);
    if (!e) return;
    const key = ++flashKeyRef.current;
    setLiveFlash({ path, key, ranges: e.ranges, anchor: e.anchor, reveal: true });
    window.setTimeout(() => setLiveFlash((f) => (f?.key === key ? null : f)), 2500);
  }, []);

  /** Bring `path` up in a view that shows working-tree agent edits (inline review or unstaged diff). */
  const focusPath = useCallback(
    (path: string, reveal = false) => {
      const existing = tabsRef.current.find(
        (t) => t.path === path && (t.mode === 'diff' || t.mode === 'code')
      );
      // A staged diff shows the index, not the agent's working-tree edits, and gets no highlights.
      if (existing?.mode === 'diff' && existing.staged) openDiff(path, false);
      else if (existing) setActiveTabId(existing.id);
      else openFile(path);
      setPausedJump(null);
      if (reveal) revealEdit(path);
    },
    [openDiff, openFile, revealEdit]
  );
  const focusAndReveal = useCallback((path: string) => focusPath(path, true), [focusPath]);

  // Watcher batch -> "what did the agent just change?" (works for any agent, CLI or built-in).
  fsChangedRef.current = (paths: string[], dirtyBefore: Set<string>) => {
    // Pull/checkout/stash/reset rewrite files too; that is git, not the agent.
    const fromGit = gitWriteActive();
    const lock =
      /(^|\/)(package-lock\.json|yarn\.lock|pnpm-lock\.yaml|Cargo\.lock|bun\.lockb?|composer\.lock|poetry\.lock|Gemfile\.lock)$/;
    let files = paths
      .map((p) => (p || '').replace(/\\/g, '/').replace(/^\.\//, ''))
      // `cmd` / `cli` / `ast` are tool notifications without a path (the watcher reports the files).
      .filter(
        (p) =>
          p &&
          !p.includes(':') &&
          !p.startsWith('.git/') &&
          p !== 'cmd' &&
          p !== 'cli' &&
          p !== 'ast' &&
          !p.endsWith('/')
      );
    const source = files.filter((p) => !lock.test(p));
    if (source.length) files = source;
    // Paths arrive oldest-first; keep the most recent ones (the last is the one to follow).
    files = files.slice(-24);
    if (!files.length) return;
    // Stamped before the follow-up `git status` starts, so a status that began earlier (and may
    // not list this file yet) can never prune its review state.
    const editedAt = Date.now();
    files.forEach((p) => {
      inflightRef.current.add(p);
      editedAtRef.current.set(p, editedAt);
    });
    while (editedAtRef.current.size > 500) {
      editedAtRef.current.delete(editedAtRef.current.keys().next().value as string);
    }

    const run = async () => {
      const results: Array<{
        path: string;
        adds: number;
        dels: number;
        ranges: Array<{ from: number; to: number }>;
        /** First changed new-side line (also set for pure deletions, which have no ranges). */
        anchor: number;
        textLf: string;
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
        const seededAt = seededRef.current.get(path);
        if (seededAt !== undefined && Date.now() - seededAt < 2500 && snapshotsRef.current.get(path) === cur) {
          // The up-front snapshot was read after this very write (it raced the watcher batch):
          // it is not a "before" state.
          snapshotsRef.current.delete(path);
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
        trimToBudget(snaps, (t) => t.length);
        if (prev === undefined || prev === cur) continue;

        // CRLF vs LF must not look like a whole-file rewrite (git stores LF, the working tree is CRLF).
        const lf = (t: string) => t.replace(/\r\n/g, '\n');
        let adds = 0;
        let dels = 0;
        let line = 0;
        let anchor = 0;
        const ranges: Array<{ from: number; to: number }> = [];
        const curLf = lf(cur);
        for (const part of diffLines(lf(prev), curLf)) {
          const n = part.count ?? 0;
          if ((part.added || part.removed) && n > 0 && !anchor) anchor = line + 1;
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
        // A deletion at the end of the file sits after the last line: clamp into the document.
        const nl = (curLf.match(/\n/g) ?? []).length;
        const lastLine = Math.max(1, curLf.endsWith('\n') ? nl : nl + 1);
        anchor = Math.min(Math.max(1, anchor), lastLine);
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
          results.push({ path, adds, dels, ranges, anchor, textLf: curLf, baseline: trusted ? eolMatched : null, total });
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

      // Fresh text for the edited files' tabs, so switching to one paints the new content at once
      // (no '' placeholder, no second read) and its inline review lines up with it.
      const cacheText = (p: string) => {
        const c = currents.get(p);
        if (c !== undefined) rememberContent(p, { file: { path: p, content: c, binary: false } });
      };
      for (const p of currents.keys()) if (tabsRef.current.some((t) => t.id === p)) cacheText(p);

      const sup = selfWriteRef.current;
      const now = Date.now();
      const ui =
        fromGit || gitWriteActive()
          ? []
          : results.filter((r) => !(sup.all > now || (sup.paths.get(r.path) ?? 0) > now));
      if (!ui.length) return;
      for (const r of ui) {
        lastEditRef.current.delete(r.path);
        lastEditRef.current.set(r.path, { ranges: r.ranges, anchor: r.anchor });
      }
      while (lastEditRef.current.size > 200) {
        lastEditRef.current.delete(lastEditRef.current.keys().next().value as string);
      }

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
      // The bar hides after a while, but a pending "Show latest" keeps it up until it is used,
      // the file is opened, or the bar is dismissed.
      pillTimerRef.current = window.setTimeout(() => setPillVisible(false), 10000);

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

      // Follow target: stay on the active file when the agent edited it, else the latest file.
      const activePath = tabsRef.current.find((t) => t.id === activeTabIdRef.current)?.path;
      const target = ui.find((r) => r.path === activePath) ?? ui[ui.length - 1];
      cacheText(target.path);
      setLiveFlash({
        path: target.path,
        key,
        ranges: target.ranges,
        anchor: target.anchor,
        text: target.textLf,
      });
      window.setTimeout(() => setLiveFlash((f) => (f?.key === key ? null : f)), 2500);
      if (followRef.current) {
        const busy = editorDirtyRef.current || Date.now() - userActivityRef.current < 4000;
        if (busy) setPausedJump(target.path);
        else if (target.path !== activePath) {
          const fs = followSwitchRef.current;
          const wait = fs.at + FOLLOW_SWITCH_DWELL_MS - Date.now();
          window.clearTimeout(fs.timer);
          if (wait <= 0) {
            fs.at = Date.now();
            focusPath(target.path);
          } else {
            // Switched moments ago: switch once more when the dwell ends, to whatever is latest.
            fs.path = target.path;
            fs.timer = window.setTimeout(() => {
              const p = fs.path;
              fs.path = null;
              if (!p || !followRef.current) return;
              if (editorDirtyRef.current || Date.now() - userActivityRef.current < 4000) {
                setPausedJump(p);
                return;
              }
              fs.at = Date.now();
              focusPath(p, true);
            }, wait);
          }
        }
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
    const next = !followEdits;
    setFollowEdits(next);
    setPref('vireo-follow-edits', next ? '1' : '0');
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

  // Capture the current text of files that are already dirty, so the first agent edit to one of
  // them diffs against the user's own work instead of git HEAD (see seededRef).
  useEffect(() => {
    if (!gitStatus?.is_repo) return;
    const todo = gitStatus.files
      .map((f) => f.path.replace(/\\/g, '/'))
      .filter(
        (p) =>
          !p.endsWith('/') &&
          !seededRef.current.has(p) &&
          !snapshotsRef.current.has(p) &&
          !inflightRef.current.has(p)
      )
      .slice(0, Math.max(0, SEED_SNAPSHOT_LIMIT - seededRef.current.size));
    if (!todo.length) return;
    let cancelled = false;
    (async () => {
      for (const p of todo) {
        if (cancelled) return;
        seededRef.current.set(p, Date.now());
        try {
          const f = await api.readFile(p);
          if (cancelled) {
            seededRef.current.delete(p); // retried by the next status update
            return;
          }
          if (f.binary || f.size > 1_500_000) continue;
          if (snapshotsRef.current.has(p) || inflightRef.current.has(p)) continue;
          seededRef.current.set(p, Date.now());
          snapshotsRef.current.set(p, f.content);
          trimToBudget(snapshotsRef.current, (t) => t.length);
        } catch {
          // deleted / unreadable: nothing to snapshot
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [gitStatus]);

  // Committed, stashed or reverted files have no pending changes left: drop them from the
  // agent-edits bar and from the review baselines (so "Undo all" can't roll back committed work).
  useEffect(() => {
    if (!gitStatus?.is_repo) return;
    const dirty = gitStatus.files.map((f) => f.path.replace(/\\/g, '/'));
    const exact = new Set(dirty);
    const dirs = dirty.filter((d) => d.endsWith('/'));
    // Only trust "clean" when this status was requested after the file's last reported change;
    // otherwise it may simply predate the edit.
    const startedAt = gitFetchRef.current.startedAt;
    const stillChanged = (p: string) =>
      exact.has(p) || dirs.some((d) => p.startsWith(d)) || (editedAtRef.current.get(p) ?? 0) >= startedAt;
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

  const openSettingsTab = useCallback(() => {
    const id = 'settings';
    setTabs((prev) => (prev.some((t) => t.id === id) ? prev : [...prev, { id, path: 'Settings', mode: 'settings' }]));
    setActiveTabId(id);
  }, []);

  const openReleaseTab = useCallback(() => {
    const id = 'release';
    setTabs((prev) => (prev.some((t) => t.id === id) ? prev : [...prev, { id, path: 'Releases', mode: 'release' }]));
    setActiveTabId(id);
  }, []);

  const openShortcutsTab = useCallback(() => {
    const id = 'shortcuts';
    setTabs((prev) =>
      prev.some((t) => t.id === id) ? prev : [...prev, { id, path: 'Keyboard Shortcuts', mode: 'shortcuts' }]
    );
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

  // Close every tab for which `shouldClose` is true; keep the active tab if it survives,
  // otherwise fall back to the last remaining tab.
  const closeTabsWhere = (shouldClose: (t: OpenTab, index: number) => boolean) => {
    const next = tabs.filter((t, i) => !shouldClose(t, i));
    for (const t of tabs) if (!next.includes(t)) contentCacheRef.current.delete(t.id);
    setTabs(next);
    if (!next.some((t) => t.id === activeTabId)) {
      setActiveTabId(next.length > 0 ? next[next.length - 1].id : null);
    }
  };

  const buildTabMenuItems = (tabId: string): ContextMenuItem[] => {
    const idx = tabs.findIndex((t) => t.id === tabId);
    return [
      { id: 'close', label: 'Close', icon: <X size={13} />, onClick: () => closeTab(tabId) },
      {
        id: 'close-others',
        label: 'Close Others',
        disabled: tabs.length < 2,
        onClick: () => closeTabsWhere((t) => t.id !== tabId),
      },
      {
        id: 'close-right',
        label: 'Close to the Right',
        disabled: idx < 0 || idx === tabs.length - 1,
        onClick: () => closeTabsWhere((_t, i) => i > idx),
      },
      {
        id: 'close-all',
        label: 'Close All',
        dividerBefore: true,
        danger: true,
        onClick: () => closeTabsWhere(() => true),
      },
    ];
  };

  const activeFilePath =
    activeTab && (activeTab.mode === 'code' || activeTab.mode === 'diff') ? activeTab.path : null;
  const fileTask = useMemo(() => activeFileTask(activeFilePath), [activeFilePath]);
  const rankedTasks = useMemo(
    () =>
      rankTasks([...tasks, ...taskPrefs.local], {
        prefs: taskPrefs,
        projectDefault: projectDefaultTask,
        activeFile: activeFilePath,
      }),
    [tasks, taskPrefs, projectDefaultTask, activeFilePath]
  );

  const launchTasks = (launches: TaskLaunch[], action: 'run' | 'stop' = 'run') => {
    if (launches.length === 0) return;
    if (action === 'run') setShowTerminal(true);
    setPendingCmd({ id: Date.now(), action, tasks: launches });
  };

  const handleRunTask = (task: TaskItem, opts?: { newTab?: boolean }) => {
    setShowRunMenu(false);
    const byId = new Map<string, TaskItem>(
      [...rankedTasks, ...(fileTask ? [fileTask] : [])].map((t) => [t.id, t])
    );
    const varCtx = { workspacePath: workspace.path, activeFile: activeFilePath };
    const launches: TaskLaunch[] = [];
    const seen = new Set<string>();
    // Dependencies start together with the task (compound tasks), each in its own tab.
    const collect = (t: TaskItem): boolean => {
      if (seen.has(t.id)) return true;
      seen.add(t.id);
      for (const depId of t.depends_on) {
        const dep = byId.get(depId);
        if (!dep) {
          showToast(`Task "${t.name}" depends on unknown task "${depId}"`, true);
          return false;
        }
        if (!collect(dep)) return false;
      }
      if (t.command.trim()) {
        const command = expandVars(t.command, varCtx, (v) =>
          showToast('${' + v + '} needs an open file', true)
        );
        if (command === null) return false;
        launches.push({
          taskId: t.id,
          name: t.name,
          command,
          cwd: t.cwd,
          env: t.env,
          newTab: opts?.newTab,
        });
      }
      return true;
    };
    if (!collect(task)) return;
    recordRun(task.id);
    launchTasks(launches);
  };

  const handleStopTask = (task: TaskItem) => {
    const ids = [task.id, ...task.depends_on];
    launchTasks(
      ids.map((taskId) => ({ taskId, name: task.name, command: '' })),
      'stop'
    );
  };

  const handleRunAdHoc = (command: string) => {
    const taskId = `adhoc:${command}`;
    launchTasks([
      { taskId, name: command.length > 24 ? `${command.slice(0, 24)}…` : command, command },
    ]);
  };

  const handleSaveTask = async (draft: TaskDraft) => {
    const cwd = draft.cwd.trim();
    if (draft.scope === 'personal') {
      const id = draft.id?.startsWith('local:') ? draft.id : `local:${slugify(draft.name)}`;
      saveLocal({
        id,
        name: draft.name.trim(),
        command: draft.command.trim(),
        source: 'personal',
        cwd,
        category: draft.category,
        score: 150,
        custom: false,
        local: true,
        env: {},
        depends_on: [],
      });
      return;
    }
    const existing = draft.id?.startsWith('custom:')
      ? tasks.find((t) => t.id === draft.id)
      : undefined;
    await api.saveCustomTask({
      id: existing?.id,
      name: draft.name.trim(),
      command: draft.command.trim(),
      cwd,
      category: draft.category,
      env: existing?.env,
    });
    refreshTasks();
  };

  const handleDeleteTask = async (task: TaskItem) => {
    if (task.local) {
      removeLocal(task.id);
      return;
    }
    try {
      await api.deleteCustomTask(task.id);
      refreshTasks();
    } catch (e) {
      showToast(e instanceof Error ? e.message : 'Failed to delete task', true);
    }
  };

  const handleTaskTabsChange = useCallback((ids: string[]) => {
    setRunningTaskIds((prev) =>
      prev.length === ids.length && prev.every((id, i) => id === ids[i]) ? prev : ids
    );
  }, []);

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
      const next = [...themes.filter((t) => t.id !== parsed.id), parsed];
      const customOnly = next.filter(
        (t) => !BUILTIN_THEMES.some((b) => b.id === t.id)
      );
      setThemes(next);
      setThemeId(parsed.id);
      setPrefs({
        'vireo-custom-themes': JSON.stringify(customOnly),
        'vireo-theme': parsed.id,
        'vireo-theme-meta': themeMetaJson(parsed),
      });
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
  const lastSavedLayoutRef = useRef({ sidebarWidth, agentWidth, terminalHeight });
  useEffect(() => {
    const prev = lastSavedLayoutRef.current;
    if (
      prev.sidebarWidth !== sidebarWidth ||
      prev.agentWidth !== agentWidth ||
      prev.terminalHeight !== terminalHeight
    ) {
      lastSavedLayoutRef.current = { sidebarWidth, agentWidth, terminalHeight };
      saveLayout({ sidebarWidth, agentWidth, terminalHeight });
    }
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

  const cycleTab = (dir: 1 | -1) => {
    const list = tabsRef.current;
    if (list.length < 2) return;
    const i = list.findIndex((t) => t.id === activeTabIdRef.current);
    setActiveTabId(list[(i + dir + list.length) % list.length].id);
  };
  const showSidebarView = (tab: SidebarTab) => {
    setShowSidebar(true);
    setSidebarTab(tab);
  };

  // Re-assigned every render so the global key handler always sees fresh closures.
  actionsRef.current = {
    quickOpen: () => setQuickOpenVisible((v) => !v),
    openSettings: openSettingsTab,
    openShortcuts: openShortcutsTab,
    saveFile: () => window.dispatchEvent(new CustomEvent('vireo:save-file')),
    toggleSidebar,
    toggleTerminal: () => setShowTerminal((v) => !v),
    toggleAgent,
    showExplorer: () => showSidebarView('explorer'),
    showSearch: () => showSidebarView('search'),
    showSourceControl: () => showSidebarView('changes'),
    openGitGraph: () => openGraphTab(),
    openReleases: openReleaseTab,
    closeTab: () => {
      const id = activeTabIdRef.current;
      if (id) closeTab(id);
    },
    closeAllTabs: () => closeTabsWhere(() => true),
    nextTab: () => cycleTab(1),
    prevTab: () => cycleTab(-1),
    zoomIn: () => adjustZoom((z) => z + ZOOM_STEP),
    zoomOut: () => adjustZoom((z) => z - ZOOM_STEP),
    zoomReset: () => adjustZoom(1.0),
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
    <AgentSettingsProvider onToast={showToast}>
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
          <kbd className="text-[10.5px] font-mono text-[var(--text-dim)]">{shortcutLabel('quickOpen')}</kbd>
        </button>

        {/* Right: actions, layout toggles, window controls */}
        <div className="flex items-center justify-end h-full min-w-0">
          <div className="flex items-center gap-0.5 pr-2">
            {/* Run Scripts */}
            <RunMenu
              open={showRunMenu}
              onOpenChange={(v) => {
                setShowRunMenu(v);
                if (v) {
                  setShowThemeMenu(false);
                  refreshTasks();
                }
              }}
              tasks={rankedTasks}
              prefs={taskPrefs}
              fileTask={fileTask}
              runningIds={runningTaskIds}
              onRun={handleRunTask}
              onRunAdHoc={handleRunAdHoc}
              onStop={handleStopTask}
              onTogglePin={(t) => setPinned(taskPrefs.pinned === t.id ? null : t.id)}
              onSave={handleSaveTask}
              onDelete={handleDeleteTask}
            />

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
                        selectTheme(th);
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
                { on: widths.sidebarVisible, toggle: toggleSidebar, Icon: PanelLeft, label: withShortcut('Toggle Sidebar', 'toggleSidebar') },
                { on: showTerminal, toggle: () => setShowTerminal((v) => !v), Icon: PanelBottom, label: withShortcut('Toggle Terminal', 'toggleTerminal') },
                { on: widths.agentVisible, toggle: toggleAgent, Icon: PanelRight, label: withShortcut('Toggle Agent Panel', 'toggleAgent') },
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
                onOpenShortcuts={openShortcutsTab}
                onOpenRelease={openReleaseTab}
                releaseActive={activeTab?.mode === 'release'}
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
              onOpenShortcuts={openShortcutsTab}
                onOpenRelease={openReleaseTab}
                releaseActive={activeTab?.mode === 'release'}
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
                    : tab.mode === 'settings'
                    ? 'Settings'
                    : tab.mode === 'shortcuts'
                    ? 'Keyboard Shortcuts'
                    : tab.mode === 'release'
                    ? 'Releases'
                    : tab.mode === 'commit-diff'
                    ? `${baseName} (${tab.commitShortHash})`
                    : tab.mode === 'compare-diff'
                    ? `${baseName} (${tab.commitShortHash}↔WT)`
                    : baseName;
                return (
                  <div
                    key={tab.id}
                    onClick={() => setActiveTabId(tab.id)}
                    onAuxClick={(e) => {
                      if (e.button === 1) {
                        e.preventDefault();
                        closeTab(tab.id);
                      }
                    }}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      setTabMenu({ x: e.clientX, y: e.clientY, tabId: tab.id });
                    }}
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
                    ) : tab.mode === 'settings' ? (
                      <Settings size={14} className="text-[var(--accent)] shrink-0" />
                    ) : tab.mode === 'shortcuts' ? (
                      <Keyboard size={14} className="text-[var(--accent)] shrink-0" />
                    ) : tab.mode === 'release' ? (
                      <Rocket size={14} className="text-[var(--accent)] shrink-0" />
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
          {tabMenu && (
            <FloatingContextMenu
              x={tabMenu.x}
              y={tabMenu.y}
              items={buildTabMenuItems(tabMenu.tabId)}
              onClose={() => setTabMenu(null)}
            />
          )}

          {/* Main Viewer Stage */}
          <div
            className="flex-1 flex flex-col min-h-0 relative"
            onWheelCapture={() => (userActivityRef.current = Date.now())}
            onKeyDownCapture={() => (userActivityRef.current = Date.now())}
            onPointerDownCapture={() => (userActivityRef.current = Date.now())}
          >
            {/* Releases stays mounted while its tab is open so choices and results survive tab switches. */}
            {tabs.some((t) => t.mode === 'release') && (
              <div className={activeTab?.mode === 'release' ? 'flex-1 flex flex-col min-h-0' : 'hidden'}>
                <Suspense
                  fallback={
                    <div className="flex-1 flex items-center justify-center text-xs text-[var(--text-dim)]">
                      Loading Releases...
                    </div>
                  }
                >
                  <ReleasePage
                    active={activeTab?.mode === 'release'}
                    fsVersion={fsVersion}
                    theme={activeTheme}
                    onOpenFile={(p) => openFile(p)}
                    onAskAgent={handleAskAgent}
                    onShowChanges={() => showSidebarView('changes')}
                    onRefreshGit={refreshGit}
                    onToast={showToast}
                  />
                </Suspense>
              </div>
            )}
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
                    {withShortcut('Quick Open', 'quickOpen')}
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
                    {withShortcut('Open Terminal', 'toggleTerminal')}
                  </button>
                </div>
              </div>
            ) : activeTab.mode === 'release' ? null : activeTab.mode === 'settings' ? (
              <Suspense
                fallback={
                  <div className="flex-1 flex items-center justify-center text-xs text-[var(--text-dim)]">
                    Loading Settings...
                  </div>
                }
              >
                <SettingsPage />
              </Suspense>
            ) : activeTab.mode === 'shortcuts' ? (
              <Suspense
                fallback={
                  <div className="flex-1 flex items-center justify-center text-xs text-[var(--text-dim)]">
                    Loading Shortcuts...
                  </div>
                }
              >
                <ShortcutsPage />
              </Suspense>
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
                      ? {
                          key: liveFlash.key,
                          ranges: liveFlash.ranges,
                          anchor: liveFlash.anchor,
                          reveal: liveFlash.reveal,
                        }
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
                  loading={!(fileData && fileData.path === activeTab.path)}
                  scrollTarget={
                    liveFlash && liveFlash.path === activeTab.path
                      ? {
                          key: liveFlash.key,
                          line: liveFlash.ranges.length ? liveFlash.ranges[0].from : liveFlash.anchor,
                          ranges: liveFlash.ranges,
                          reveal: liveFlash.reveal,
                          text: liveFlash.text,
                        }
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
                  onFocus={focusAndReveal}
                />
                <div className="flex items-center gap-1.5 shrink-0">
                  {pausedJump && (
                    <button
                      onClick={() => focusPath(pausedJump, true)}
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
                    onTaskTabsChange={handleTaskTabsChange}
                    launcherSignal={launcherSignal}
                    maximized={terminalMaximized}
                    onToggleMaximize={() => setTerminalMaximized((v) => !v)}
                    onSendOutputToAgent={handleAskAgent}
                    onClosePanel={() => setShowTerminal(false)}
                    visible={showTerminal}
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
              <Suspense fallback={null}>
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
                onOpenSettings={openSettingsTab}
              />
              </Suspense>
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
          <div className="group flex items-stretch" title={`Zoom (${shortcutLabel('zoomIn')} / ${shortcutLabel('zoomOut')} / ${shortcutLabel('zoomReset')})`}>
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
      {toast && <Toast key={toast.id} toast={toast} onDismiss={() => setToast((cur) => (cur?.id === toast.id ? null : cur))} />}

      {/* Quick Open Modal */}
      {quickOpenVisible && (
        <Suspense fallback={null}>
          <QuickOpen
            open={quickOpenVisible}
            files={allFiles}
            themes={themes}
            tasks={rankedTasks}
            onClose={() => setQuickOpenVisible(false)}
            onSelectFile={(p) => openFile(p)}
            onSelectTheme={(id) => selectTheme(id)}
            onRunTask={handleRunTask}
          />
        </Suspense>
      )}
    </div>
    </AgentSettingsProvider>
  );
}

export default App;
