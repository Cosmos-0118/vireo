import { api } from '../api';

/** Drag payload for a workspace file/folder dragged out of Vireo's own panels. */
export const PATH_MIME = 'application/x-vireo-path';

export interface DragPath {
  /** Workspace-relative path. */
  rel: string;
  /** Absolute path on disk (what a terminal needs). */
  abs: string;
}

export function setPathDrag(e: { dataTransfer: DataTransfer }, p: DragPath) {
  e.dataTransfer.setData(PATH_MIME, JSON.stringify(p));
  e.dataTransfer.setData('text/plain', p.abs);
  e.dataTransfer.effectAllowed = 'copy';
}

/** True when the drag carries something we can use (a Vireo path or OS files). */
export function isUsableDrag(dt: DataTransfer | null): boolean {
  if (!dt) return false;
  const types = Array.from(dt.types);
  return types.includes(PATH_MIME) || types.includes('Files');
}

export function readDrop(dt: DataTransfer | null): { paths: DragPath[]; files: File[] } {
  const paths: DragPath[] = [];
  const raw = dt?.getData(PATH_MIME);
  if (raw) {
    try {
      const p = JSON.parse(raw) as DragPath;
      if (p && typeof p.rel === 'string' && typeof p.abs === 'string') paths.push(p);
    } catch {
      // ignore malformed payloads
    }
  }
  return { paths, files: Array.from(dt?.files ?? []) };
}

export function fileExt(file: File): string {
  const dot = file.name.lastIndexOf('.');
  if (dot > 0 && dot < file.name.length - 1) return file.name.slice(dot + 1);
  return (file.type.split('/')[1] || 'bin').replace('jpeg', 'jpg');
}

/** Saves an OS file to disk (the browser never exposes its real path) and returns that path. */
export async function uploadDropped(file: File): Promise<string> {
  const stem = file.name.replace(/\.[^.]*$/, '');
  return (await api.saveAttachment(file, fileExt(file), stem)).path;
}

/**
 * Quotes a path for pasting into a shell prompt when it needs it. Windows paths (drive letter
 * or UNC) target PowerShell, everything else a POSIX shell. Both use single quotes, inside
 * which nothing (including backslashes, `$` and backticks) is interpreted.
 */
export function shellQuote(p: string): string {
  if (/^(?:[A-Za-z]:[\\/]|\\\\)/.test(p)) {
    if (/^[\w.:\\/+-]+$/.test(p)) return p;
    // PowerShell also treats the typographic single quotes as quote characters.
    return `'${p.replace(/['‘’‚‛]/g, (q) => q + q)}'`;
  }
  if (/^[\w@%+=:,./-]+$/.test(p)) return p;
  return `'${p.replace(/'/g, `'\\''`)}'`;
}
