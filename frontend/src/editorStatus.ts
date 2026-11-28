import { useSyncExternalStore } from 'react';

/**
 * Cursor / selection of the focused code editor, for the status bar. Kept outside React state so
 * a keystroke re-renders only the status bar item, not the app.
 */
export interface EditorStatus {
  path: string;
  line: number;
  col: number;
  /** Characters selected across all ranges (0 = no selection). */
  selChars: number;
  /** Lines touched by the selection (0 = no selection). */
  selLines: number;
  /** Number of cursors / selections. */
  cursors: number;
  /** Language display name ("TypeScript JSX"). */
  language: string;
}

let current: EditorStatus | null = null;
let gotoLine: (() => void) | null = null;
const listeners = new Set<() => void>();

const same = (a: EditorStatus | null, b: EditorStatus | null) =>
  a === b ||
  (!!a &&
    !!b &&
    a.path === b.path &&
    a.line === b.line &&
    a.col === b.col &&
    a.selChars === b.selChars &&
    a.selLines === b.selLines &&
    a.cursors === b.cursors &&
    a.language === b.language);

export function publishEditorStatus(next: EditorStatus, openGotoLine: () => void): void {
  gotoLine = openGotoLine;
  if (same(current, next)) return;
  current = next;
  listeners.forEach((l) => l());
}

/** Called when an editor goes away; ignored if another editor has published since. */
export function clearEditorStatus(path: string): void {
  if (current?.path !== path) return;
  current = null;
  gotoLine = null;
  listeners.forEach((l) => l());
}

export function openGotoLine(): void {
  gotoLine?.();
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

export function useEditorStatus(): EditorStatus | null {
  return useSyncExternalStore(subscribe, () => current);
}
