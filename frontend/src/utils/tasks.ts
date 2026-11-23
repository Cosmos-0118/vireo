import { useCallback, useEffect, useState } from 'react';
import type { TaskItem } from '../types';

/** Per-workspace, per-machine task state: pinned default, run history, personal tasks. */
export interface TaskPrefs {
  pinned: string | null;
  history: Record<string, { lastRunAt: number; runCount: number }>;
  local: TaskItem[];
}

const EMPTY_PREFS: TaskPrefs = { pinned: null, history: {}, local: [] };
const storageKey = (ws: string) => `vireo-tasks::${ws}`;

function loadPrefs(ws: string): TaskPrefs {
  if (!ws) return EMPTY_PREFS;
  try {
    const raw = localStorage.getItem(storageKey(ws));
    if (!raw) return EMPTY_PREFS;
    const p = JSON.parse(raw) as Partial<TaskPrefs>;
    return {
      pinned: typeof p.pinned === 'string' ? p.pinned : null,
      history: p.history && typeof p.history === 'object' ? p.history : {},
      local: Array.isArray(p.local) ? p.local : [],
    };
  } catch {
    return EMPTY_PREFS;
  }
}

export function useTaskPrefs(workspacePath: string) {
  const [prefs, setPrefs] = useState<TaskPrefs>(() => loadPrefs(workspacePath));

  useEffect(() => {
    setPrefs(loadPrefs(workspacePath));
  }, [workspacePath]);

  const update = useCallback(
    (fn: (p: TaskPrefs) => TaskPrefs) => {
      setPrefs((prev) => {
        const next = fn(prev);
        if (workspacePath) {
          try {
            localStorage.setItem(storageKey(workspacePath), JSON.stringify(next));
          } catch {
            /* storage full or unavailable: keep in-memory state */
          }
        }
        return next;
      });
    },
    [workspacePath]
  );

  const setPinned = useCallback(
    (id: string | null) => update((p) => ({ ...p, pinned: id })),
    [update]
  );
  const recordRun = useCallback(
    (id: string) =>
      update((p) => {
        const prev = p.history[id];
        return {
          ...p,
          history: {
            ...p.history,
            [id]: { lastRunAt: Date.now(), runCount: (prev?.runCount ?? 0) + 1 },
          },
        };
      }),
    [update]
  );
  const saveLocal = useCallback(
    (task: TaskItem) =>
      update((p) => ({
        ...p,
        local: [...p.local.filter((t) => t.id !== task.id), task],
      })),
    [update]
  );
  const removeLocal = useCallback(
    (id: string) =>
      update((p) => ({
        ...p,
        pinned: p.pinned === id ? null : p.pinned,
        local: p.local.filter((t) => t.id !== id),
      })),
    [update]
  );

  return { prefs, setPinned, recordRun, saveLocal, removeLocal };
}

const HOUR = 3600_000;
const DAY = 24 * HOUR;

/** Frequency x recency of runs in this workspace: 0 (never) or 50..200. */
export function frecency(h: TaskPrefs['history'][string] | undefined, now = Date.now()): number {
  if (!h) return 0;
  const age = now - h.lastRunAt;
  const recency = age < HOUR ? 120 : age < DAY ? 80 : age < 7 * DAY ? 40 : 0;
  const frequency = Math.min(h.runCount, 6) * 5;
  return Math.min(200, 50 + recency + frequency);
}

export interface RankContext {
  prefs: TaskPrefs;
  /** `defaultTask` from .vireo/tasks.json. */
  projectDefault: string | null;
  /** Workspace-relative path of the focused editor file. */
  activeFile: string | null;
  now?: number;
}

/** Combines backend score with pin, project default, frecency and active-file affinity. */
export function rankTasks(tasks: TaskItem[], ctx: RankContext): TaskItem[] {
  const scored = tasks.map((t) => {
    let s = t.score;
    s += frecency(ctx.prefs.history[t.id], ctx.now);
    if (t.cwd && ctx.activeFile && ctx.activeFile.startsWith(`${t.cwd}/`)) s += 35;
    if (ctx.projectDefault === t.id) s += 10_000;
    if (ctx.prefs.pinned === t.id) s += 100_000;
    return { t, s };
  });
  scored.sort((a, b) => b.s - a.s);
  return scored.map((x) => x.t);
}

export function slugify(s: string): string {
  return (
    s
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '') || 'task'
  );
}

// ---------------------------------------------------------------------------
// Variables: ${workspaceFolder} ${file} ${relativeFile} ${fileDirname}
// ${fileBasename} ${fileBasenameNoExtension} ${input:Label:default}
// ---------------------------------------------------------------------------

export interface VarContext {
  workspacePath: string;
  /** Workspace-relative path of the focused editor file, if any. */
  activeFile: string | null;
}

const dirname = (p: string) => (p.includes('/') ? p.slice(0, p.lastIndexOf('/')) : '.');
const basename = (p: string) => p.slice(p.lastIndexOf('/') + 1);

/**
 * Expands task variables. Returns null when the user cancels an `${input:...}` prompt, or
 * when a file variable is used with no file open (reported through `onMissing`).
 */
export function expandVars(
  text: string,
  ctx: VarContext,
  onMissing?: (variable: string) => void
): string | null {
  let cancelled = false;
  let missing = false;
  const file = ctx.activeFile;
  const out = text.replace(/\$\{([^}]+)\}/g, (whole, expr: string) => {
    if (cancelled || missing) return whole;
    if (expr === 'workspaceFolder') return ctx.workspacePath;
    if (expr.startsWith('input:')) {
      const [label, ...rest] = expr.slice(6).split(':');
      const answer = window.prompt(label || 'Value', rest.join(':'));
      if (answer === null) {
        cancelled = true;
        return whole;
      }
      return answer;
    }
    const fileVars: Record<string, (f: string) => string> = {
      relativeFile: (f) => f,
      file: (f) => `${ctx.workspacePath.replace(/[\\/]+$/, '')}/${f}`,
      fileDirname: dirname,
      fileBasename: basename,
      fileBasenameNoExtension: (f) => basename(f).replace(/\.[^.]*$/, ''),
    };
    const fn = fileVars[expr];
    if (!fn) return whole;
    if (!file) {
      missing = true;
      onMissing?.(expr);
      return whole;
    }
    return fn(file);
  });
  return cancelled || missing ? null : out;
}

const quote = (p: string) => (/\s/.test(p) ? `"${p}"` : p);

/** A contextual "run this file" task for the focused editor tab, or null. */
export function activeFileTask(activeFile: string | null): TaskItem | null {
  if (!activeFile) return null;
  const f = activeFile;
  const name = basename(f);
  let command: string | null = null;
  let label = '';
  let category: TaskItem['category'] = 'run';
  if (/\.(test|spec)\.[cm]?[jt]sx?$/.test(name)) {
    command = `npx vitest run ${quote(f)}`;
    label = `Run tests in ${name}`;
    category = 'test';
  } else if (/^test_.*\.py$|_test\.py$/.test(name)) {
    command = `pytest ${quote(f)}`;
    label = `Run tests in ${name}`;
    category = 'test';
  } else if (/_test\.go$/.test(name)) {
    command = `go test ./${quote(dirname(f))}`;
    label = `Run tests in ${name}`;
    category = 'test';
  } else if (/\.py$/.test(name)) {
    command = `python ${quote(f)}`;
  } else if (/\.ps1$/.test(name)) {
    command = `& ${quote(`./${f}`)}`;
  } else if (/\.sh$/.test(name)) {
    command = `bash ${quote(f)}`;
  } else if (/\.[cm]?js$/.test(name)) {
    command = `node ${quote(f)}`;
  } else if (/\.[cm]?ts$/.test(name) && !name.endsWith('.d.ts')) {
    command = `npx tsx ${quote(f)}`;
  } else if (name === 'main.go') {
    command = `go run ${quote(`./${dirname(f)}`)}`;
  }
  if (!command) return null;
  return {
    id: `file:${f}`,
    name: label || `Run ${name}`,
    command,
    source: 'active file',
    cwd: '',
    category,
    score: 0,
    custom: false,
    env: {},
    depends_on: [],
  };
}
