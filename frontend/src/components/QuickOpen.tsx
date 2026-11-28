import { MagnifyingGlass } from '@phosphor-icons/react';
import { EmptyRow } from './ui/EmptyState';
import React, { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Palette, Play, Search } from 'lucide-react';
import { FileIcon } from './FileIcon';
import type { LiteTheme } from '../themes';
import type { TaskItem } from '../types';
import { createFuzzySearcher, type FuzzySearcher } from '../utils/fuzzy';

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

const MAX_FILES = 40;

// One searcher per file list, shared across opens so the lowercase index (and the
// incremental-narrowing cache) is built once rather than on every keystroke / open.
const searchers = new WeakMap<string[], FuzzySearcher>();
const searcherFor = (files: string[]): FuzzySearcher => {
  let s = searchers.get(files);
  if (!s) {
    s = createFuzzySearcher(files);
    searchers.set(files, s);
  }
  return s;
};

const Row = memo(function Row({
  item,
  index,
  active,
  onPick,
}: {
  item: PaletteEntry;
  index: number;
  active: boolean;
  onPick: (index: number) => void;
}) {
  return (
    <div
      data-active={active || undefined}
      onClick={() => onPick(index)}
      className={`flex items-center justify-between px-3 py-1.5 text-xs cursor-pointer ${
        active
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
        <span className="text-[10px] opacity-75 font-mono ml-2 truncate">{item.subtitle}</span>
      )}
    </div>
  );
});

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
  const listRef = useRef<HTMLDivElement>(null);

  // Focus before first paint (no 10ms timer). The modal is mounted fresh on every open,
  // so state starts clean; only reset when `open` flips while staying mounted.
  useLayoutEffect(() => {
    if (!open) return;
    setQuery('');
    setIdx(0);
    inputRef.current?.focus();
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

    return searcherFor(files)
      .search(q, MAX_FILES)
      .map((f) => ({ kind: 'file', label: f, value: f }));
  }, [query, files, themes, tasks]);

  // Keep the highlighted row visible while navigating with the keyboard.
  useEffect(() => {
    listRef.current?.querySelector('[data-active]')?.scrollIntoView({ block: 'nearest' });
  }, [idx, items]);

  const latest = useRef({ items, onClose, onSelectFile, onSelectTheme, onRunTask });
  latest.current = { items, onClose, onSelectFile, onSelectTheme, onRunTask };

  const pick = useCallback((index: number) => {
    const { items: list, onClose: close, onSelectFile: file, onSelectTheme: theme, onRunTask: run } =
      latest.current;
    const item = list[index];
    if (!item) return;
    close();
    if (item.kind === 'file') file(item.value);
    else if (item.kind === 'theme') theme(item.value);
    else run(item.task);
  }, []);

  if (!open) return null;

  const active = Math.min(idx, Math.max(0, items.length - 1));

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
            onChange={(e) => {
                setQuery(e.target.value);
                setIdx(0);
              }}
            onKeyDown={(e) => {
              if (e.key === 'Escape') onClose();
              else if (e.key === 'ArrowDown') {
                e.preventDefault();
                setIdx(Math.min(active + 1, Math.max(0, items.length - 1)));
              } else if (e.key === 'ArrowUp') {
                e.preventDefault();
                setIdx(Math.max(0, active - 1));
              } else if (e.key === 'Enter' && items[active]) {
                e.preventDefault();
                pick(active);
              }
            }}
            placeholder="Open file by name... (prefix with '>' for themes & tasks)"
            className="flex-1 bg-transparent text-sm text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none"
          />
        </div>
        <div ref={listRef} className="max-h-[340px] overflow-y-auto py-1">
          {items.length === 0 ? (
            <EmptyRow icon={MagnifyingGlass} className="!px-4 !py-3">
              No matching files or commands
            </EmptyRow>
          ) : (
            items.map((item, i) => (
              <Row key={`${item.kind}-${item.label}`} item={item} index={i} active={i === active} onPick={pick} />
            ))
          )}
        </div>
      </div>
    </div>
  );
};
