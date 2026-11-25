// Persistent UI preferences backed by localStorage (for synchronous reads) and
// the backend's on-disk prefs file (/api/prefs) so settings survive desktop
// restarts across ports and stay shared across windows.

declare global {
  interface Window {
    __VIREO_PREFS__?: Record<string, string | null>;
  }
}

const synced: Record<string, string | null> =
  typeof window !== 'undefined' &&
  window.__VIREO_PREFS__ &&
  typeof window.__VIREO_PREFS__ === 'object'
    ? { ...window.__VIREO_PREFS__ }
    : {};

const pendingSet = new Map<string, string>();
const pendingRemove = new Set<string>();
let flushTimer: number | null = null;

function sendPatch(set: Record<string, string>, remove: string[]): void {
  if (Object.keys(set).length === 0 && remove.length === 0) return;
  try {
    void fetch('/api/prefs', {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ set, remove }),
      keepalive: true,
    }).catch(() => {});
  } catch {
    // ignore in non-browser environments
  }
}

export function flushPrefs(): void {
  if (flushTimer !== null) {
    window.clearTimeout(flushTimer);
    flushTimer = null;
  }
  if (pendingSet.size === 0 && pendingRemove.size === 0) return;
  const set: Record<string, string> = {};
  for (const [k, v] of pendingSet) set[k] = v;
  const remove = Array.from(pendingRemove);
  pendingSet.clear();
  pendingRemove.clear();
  sendPatch(set, remove);
}

function scheduleFlush(debounceMs: number): void {
  if (debounceMs <= 0) {
    flushPrefs();
    return;
  }
  if (flushTimer !== null) window.clearTimeout(flushTimer);
  flushTimer = window.setTimeout(flushPrefs, debounceMs);
}

if (typeof window !== 'undefined') {
  window.addEventListener('pagehide', flushPrefs);
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') flushPrefs();
  });
}

export function hasPersistedPref(key: string): boolean {
  return Object.prototype.hasOwnProperty.call(synced, key);
}

export function setPrefs(entries: Record<string, string>, opts?: { debounceMs?: number }): void {
  let dirty = false;
  for (const [key, value] of Object.entries(entries)) {
    try {
      localStorage.setItem(key, value);
    } catch {
      // ignore (private mode / quota)
    }
    if (synced[key] === value && !pendingRemove.has(key) && !pendingSet.has(key)) {
      continue;
    }
    synced[key] = value;
    pendingRemove.delete(key);
    pendingSet.set(key, value);
    dirty = true;
  }
  if (dirty) scheduleFlush(opts?.debounceMs ?? 0);
}

export function setPref(key: string, value: string, opts?: { debounceMs?: number }): void {
  setPrefs({ [key]: value }, opts);
}

export function removePref(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    // ignore
  }
  if (synced[key] === null && !pendingSet.has(key) && !pendingRemove.has(key)) {
    return;
  }
  synced[key] = null;
  pendingSet.delete(key);
  pendingRemove.add(key);
  scheduleFlush(0);
}
