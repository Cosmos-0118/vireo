import React, { useEffect, useMemo, useRef, useState } from 'react';
import { Palette, Play, Search } from 'lucide-react';
import { FileIcon } from './FileIcon';
import type { LiteTheme } from '../themes';
import type { TaskItem } from '../types';

interface QuickOpenProps {
  open: boolean;
  files: string[];
  themes: LiteTheme[];
  tasks: TaskItem[];
  onClose: () => void;
  onSelectFile: (path: string) => void;
  onSelectTheme: (themeId: string) => void;
  onRunTask: (task: TaskItem) => void;
}

type PaletteEntry =
  | { kind: 'file'; label: string; value: string }
  | { kind: 'theme'; label: string; value: string }
  | { kind: 'task'; label: string; subtitle: string; task: TaskItem };

export const QuickOpen: React.FC<QuickOpenProps> = ({
  open,
  files,
  themes,
  tasks,
  onClose,
  onSelectFile,
  onSelectTheme,
  onRunTask,
}) => {
  const [query, setQuery] = useState('');
  const [idx, setIdx] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setQuery('');
      setIdx(0);
      setTimeout(() => inputRef.current?.focus(), 10);
    }
  }, [open]);

  const items = useMemo<PaletteEntry[]>(() => {
    const q = query.trim().toLowerCase();
    if (q.startsWith('>')) {
      const sub = q.slice(1).trim();
      const out: PaletteEntry[] = [];
      for (const t of tasks) {
        if (!sub || t.name.toLowerCase().includes(sub) || t.command.toLowerCase().includes(sub)) {
          out.push({ kind: 'task', label: `Run: ${t.name}`, subtitle: `${t.cwd ? `${t.cwd}/ · ` : ''}${t.command}  ·  ${t.source}`, task: t });
        }
      }
      for (const th of themes) {
        if (!sub || th.name.toLowerCase().includes(sub) || 'theme'.includes(sub)) {
          out.push({ kind: 'theme', label: `Theme: ${th.name}`, value: th.id });
        }
      }
      return out.slice(0, 30);
    }

    const filtered = q
      ? files.filter((f) => {
          const fl = f.toLowerCase();
          if (fl.includes(q)) return true;
          // Simple subsequence fuzzy match
          let qi = 0;
          for (let i = 0; i < fl.length && qi < q.length; i++) {
            if (fl[i] === q[qi]) qi++;
          }
          return qi === q.length;
        })
      : files;

    return filtered.slice(0, 40).map((f) => ({ kind: 'file', label: f, value: f }));
  }, [query, files, themes, tasks]);

  useEffect(() => {
    setIdx(0);
  }, [query]);

  if (!open) return null;

  const activate = (item: PaletteEntry) => {
    onClose();
    if (item.kind === 'file') onSelectFile(item.value);
    else if (item.kind === 'theme') onSelectTheme(item.value);
    else if (item.kind === 'task') onRunTask(item.task);
  };

  return (
    <div
      onClick={onClose}
      className="fixed inset-0 z-50 bg-black/50 flex items-start justify-center pt-[12vh]"
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="w-[560px] max-w-[92vw] rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl overflow-hidden"
      >
        <div className="flex items-center gap-2 px-3 py-2.5 border-b border-[var(--border)]">
          <Search size={15} className="text-[var(--text-muted)]" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') onClose();
              else if (e.key === 'ArrowDown') {
                e.preventDefault();
                setIdx((i) => Math.min(i + 1, Math.max(0, items.length - 1)));
              } else if (e.key === 'ArrowUp') {
                e.preventDefault();
                setIdx((i) => Math.max(0, i - 1));
              } else if (e.key === 'Enter' && items[idx]) {
                e.preventDefault();
                activate(items[idx]);
              }
            }}
            placeholder="Open file by name... (prefix with '>' for themes & tasks)"
            className="flex-1 bg-transparent text-sm text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none"
          />
        </div>
        <div className="max-h-[340px] overflow-y-auto py-1">
          {items.length === 0 ? (
            <div className="px-4 py-6 text-center text-xs text-[var(--text-dim)]">
              No matching files or commands.
            </div>
          ) : (
            items.map((item, i) => (
              <div
                key={`${item.kind}-${item.label}`}
                onClick={() => activate(item)}
                className={`flex items-center justify-between px-3 py-1.5 text-xs cursor-pointer ${
                  i === idx
                    ? 'bg-[var(--accent)] text-[var(--accent-fg)]'
                    : 'text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
                }`}
              >
                <div className="flex items-center gap-2 min-w-0">
                  {item.kind === 'file' && <FileIcon path={item.label} size={14} />}
                  {item.kind === 'theme' && <Palette size={13} className="shrink-0" />}
                  {item.kind === 'task' && <Play size={13} className="shrink-0" />}
                  <span className="font-mono truncate">{item.label}</span>
                </div>
                {item.kind === 'task' && (
                  <span className="text-[10px] opacity-75 font-mono ml-2 truncate">
                    {item.subtitle}
                  </span>
                )}
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
};
