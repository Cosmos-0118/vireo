import React, { useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { matchesAction } from '../keybindings';
import {
  AlertTriangle,
  ChevronDown,
  ExternalLink,
  Laptop,
  Pencil,
  Play,
  Plus,
  RotateCw,
  Square,
  Star,
  Terminal,
  Trash2,
  Users,
  X,
} from 'lucide-react';
import type { TaskCategory, TaskItem } from '../types';
import type { TaskPrefs } from '../utils/tasks';

export interface TaskDraft {
  id?: string;
  name: string;
  command: string;
  cwd: string;
  category: TaskCategory;
  scope: 'project' | 'personal';
}

interface RunMenuProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** All runnable tasks, best first (pin / frecency / backend score already applied). */
  tasks: TaskItem[];
  prefs: TaskPrefs;
  /** Contextual "run the focused file" task, if any. */
  fileTask: TaskItem | null;
  /** Task ids that currently own a terminal tab. */
  runningIds: string[];
  onRun: (task: TaskItem, opts?: { newTab?: boolean }) => void;
  onRunAdHoc: (command: string) => void;
  onStop: (task: TaskItem) => void;
  onTogglePin: (task: TaskItem) => void;
  onSave: (draft: TaskDraft) => Promise<void>;
  onDelete: (task: TaskItem) => Promise<void>;
}

const CATEGORY_LABEL: Record<TaskCategory, string> = {
  run: 'Run & Dev',
  build: 'Build',
  test: 'Test',
  check: 'Check & Lint',
  other: 'Other',
};
const CATEGORY_ORDER: TaskCategory[] = ['run', 'build', 'test', 'check', 'other'];
const COMMAND_VARS = ['${relativeFile}', '${workspaceFolder}', '${input:Label:default}'];
const fieldLabel = 'block text-[11px] font-medium text-[var(--text-muted)]';
const fieldInput =
  'w-full px-2.5 py-2 text-xs rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none focus:border-[var(--accent)] placeholder:text-[var(--text-dim)]';

const emptyDraft = (cwd = ''): TaskDraft => ({
  name: '',
  command: '',
  cwd,
  category: 'run',
  scope: 'project',
});

const SectionLabel: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div className="px-3 pt-2 pb-1 text-[11px] uppercase font-semibold tracking-wider text-[var(--text-dim)]">
    {children}
  </div>
);

export const RunMenu: React.FC<RunMenuProps> = ({
  open,
  onOpenChange,
  tasks,
  prefs,
  fileTask,
  runningIds,
  onRun,
  onRunAdHoc,
  onStop,
  onTogglePin,
  onSave,
  onDelete,
}) => {
  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const commandRef = useRef<HTMLInputElement>(null);
  const [filter, setFilter] = useState('');
  const [idx, setIdx] = useState(0);
  const [draft, setDraft] = useState<TaskDraft | null>(null);
  const [saving, setSaving] = useState(false);
  const [formError, setFormError] = useState('');

  const primary = tasks[0] ?? null;
  const running = (t: TaskItem | null) => !!t && runningIds.includes(t.id);

  useEffect(() => {
    if (!open) return;
    setFilter('');
    setIdx(0);
    setDraft(null);
    setFormError('');
    setTimeout(() => inputRef.current?.focus(), 10);
    const onDown = (e: MouseEvent) => {
      if ((e.target as HTMLElement | null)?.closest?.('[data-run-modal]')) return;
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) onOpenChange(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open, onOpenChange]);

  // F5 runs the primary task (restarts it when it is already running).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!matchesAction(e, 'runPrimary')) return;
      e.preventDefault();
      // F5 must never reload the app; only run when there is a task and the terminal isn't focused (htop etc.).
      if (!primary || (e.target as HTMLElement | null)?.closest?.('.xterm')) return;
      onRun(primary);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [primary, onRun]);

  const q = filter.trim().toLowerCase();
  const matches = (t: TaskItem) =>
    !q ||
    t.name.toLowerCase().includes(q) ||
    t.command.toLowerCase().includes(q) ||
    t.source.toLowerCase().includes(q);

  const sections = useMemo(() => {
    const filtered = tasks.filter(matches);
    const used = new Set<string>();
    const take = (list: TaskItem[]) => {
      const out = list.filter((t) => !used.has(t.id));
      out.forEach((t) => used.add(t.id));
      return out;
    };
    const out: { key: string; title: string; items: TaskItem[] }[] = [];
    const file = fileTask && matches(fileTask) ? [fileTask] : [];
    if (file.length) out.push({ key: 'file', title: 'Active File', items: take(file) });
    if (!q) {
      const recent = filtered
        .filter((t) => t.id === prefs.pinned || prefs.history[t.id])
        .sort(
          (a, b) =>
            Number(b.id === prefs.pinned) - Number(a.id === prefs.pinned) ||
            (prefs.history[b.id]?.lastRunAt ?? 0) - (prefs.history[a.id]?.lastRunAt ?? 0)
        )
        .slice(0, 3);
      if (recent.length) out.push({ key: 'recent', title: 'Pinned / Recent', items: take(recent) });
    }
    const custom = take(filtered.filter((t) => t.custom || t.local));
    if (custom.length) out.push({ key: 'custom', title: 'Custom Commands', items: custom });
    for (const cat of CATEGORY_ORDER) {
      const items = take(filtered.filter((t) => t.category === cat));
      if (items.length) out.push({ key: cat, title: CATEGORY_LABEL[cat], items });
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tasks, fileTask, q, prefs]);

  const flat = useMemo(() => sections.flatMap((s) => s.items), [sections]);
  useEffect(() => setIdx(0), [q]);

  const workspaceDirs = useMemo(
    () => Array.from(new Set(tasks.map((t) => t.cwd).filter(Boolean))).sort(),
    [tasks]
  );

  const openForm = (d: TaskDraft) => {
    setFormError('');
    setDraft(d);
  };
  const draftFrom = (t: TaskItem, edit: boolean): TaskDraft => ({
    id: edit ? t.id : undefined,
    name: edit ? t.name : `${t.name} (custom)`,
    command: t.command,
    cwd: t.cwd,
    category: t.category,
    scope: t.local ? 'personal' : 'project',
  });

  const submitDraft = async () => {
    if (!draft) return;
    if (!draft.name.trim() || !draft.command.trim()) {
      setFormError('Name and command are required.');
      return;
    }
    setSaving(true);
    try {
      await onSave(draft);
      setDraft(null);
    } catch (e) {
      setFormError(e instanceof Error ? e.message : 'Failed to save');
    } finally {
      setSaving(false);
    }
  };

  useEffect(() => {
    if (!draft) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setDraft(null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [draft]);

  const insertVar = (v: string) => {
    if (!draft) return;
    const el = commandRef.current;
    const s = el?.selectionStart ?? draft.command.length;
    const e = el?.selectionEnd ?? s;
    setDraft({ ...draft, command: draft.command.slice(0, s) + v + draft.command.slice(e) });
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(s + v.length, s + v.length);
    });
  };

  const runAndClose = (t: TaskItem, opts?: { newTab?: boolean }) => {
    onOpenChange(false);
    onRun(t, opts);
  };

  const onFilterKey = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setIdx((i) => Math.min(flat.length - 1, i + 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setIdx((i) => Math.max(0, i - 1));
    } else if (e.key === 'Escape') {
      onOpenChange(false);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const raw = filter.trim();
      if (e.ctrlKey || e.metaKey) {
        if (raw) openForm({ ...emptyDraft(), name: raw, command: raw });
      } else if (flat[idx]) {
        runAndClose(flat[idx]);
      } else if (raw) {
        onOpenChange(false);
        onRunAdHoc(raw);
      }
    }
  };

  useEffect(() => {
    rootRef.current
      ?.querySelector<HTMLElement>(`[data-run-idx="${idx}"]`)
      ?.scrollIntoView({ block: 'nearest' });
  }, [idx]);

  const iconBtn =
    'p-1 rounded text-[var(--text-dim)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-active)] cursor-pointer';

  let rowIndex = -1;
  const renderRow = (t: TaskItem) => {
    rowIndex += 1;
    const i = rowIndex;
    const pinned = prefs.pinned === t.id;
    const isRunning = running(t);
    return (
      <div
        key={`${t.id}`}
        data-run-idx={i}
        onMouseEnter={() => setIdx(i)}
        className={`group flex items-center gap-1 px-3 py-1.5 cursor-pointer ${
          i === idx ? 'bg-[var(--bg-hover)]' : ''
        }`}
        onClick={() => runAndClose(t)}
      >
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-1.5 text-[13px] text-[var(--text-primary)]">
            {isRunning && (
              <span className="w-1.5 h-1.5 rounded-full bg-[var(--added-fg)] animate-pulse shrink-0" />
            )}
            <span className="truncate">{t.name}</span>
            {pinned && <Star size={11} className="shrink-0 fill-current text-[var(--accent)]" />}
            {t.depends_on.length > 0 && (
              <span className="text-[10px] text-[var(--text-dim)] shrink-0">
                +{t.depends_on.length} tasks
              </span>
            )}
          </div>
          <div className="flex items-center gap-1.5 text-[11px] font-mono text-[var(--text-dim)]">
            <span className="truncate">{t.command || t.depends_on.join(' + ')}</span>
          </div>
        </div>
        <div className="hidden group-hover:flex items-center shrink-0" onClick={(e) => e.stopPropagation()}>
          <button
            className={iconBtn}
            title={pinned ? 'Unpin as default' : 'Pin as default run task'}
            onClick={() => onTogglePin(t)}
          >
            <Star size={12} className={pinned ? 'fill-current' : ''} />
          </button>
          <button className={iconBtn} title="Run in a new terminal tab" onClick={() => runAndClose(t, { newTab: true })}>
            <ExternalLink size={12} />
          </button>
          {t.id.startsWith('file:') ? null : t.custom || t.local ? (
            <>
              <button className={iconBtn} title="Edit" onClick={() => openForm(draftFrom(t, true))}>
                <Pencil size={12} />
              </button>
              <button
                className={iconBtn}
                title="Delete"
                onClick={() => {
                  void onDelete(t);
                }}
              >
                <Trash2 size={12} />
              </button>
            </>
          ) : (
            <button className={iconBtn} title="Customize… (save an editable copy)" onClick={() => openForm(draftFrom(t, false))}>
              <Pencil size={12} />
            </button>
          )}
          {isRunning && (
            <button className={iconBtn} title="Stop" onClick={() => onStop(t)}>
              <Square size={12} />
            </button>
          )}
        </div>
      </div>
    );
  };

  const inputCls =
    'w-full h-7 px-2 rounded-md border border-[var(--border)] bg-[var(--bg-input)] text-[12.5px] text-[var(--text-primary)] outline-none focus:border-[var(--accent)]';

  return (
    <div className="relative flex items-center" data-no-drag ref={rootRef}>
      {/* Split pill: primary action + menu toggle */}
      <div
        className={`flex items-center gap-0.5 h-7 pr-0.5 rounded-md transition-colors ${
          open ? 'bg-[var(--bg-active)]' : 'hover:bg-[var(--bg-hover)]'
        }`}
      >
        <button
          disabled={!primary}
          onClick={() => primary && onRun(primary)}
          className="h-7 pl-2 pr-1.5 text-[12.5px] flex items-center gap-1.5 cursor-pointer text-[var(--text-muted)] hover:text-[var(--text-primary)] disabled:opacity-50 disabled:cursor-default max-w-[220px]"
          title={
            primary
              ? `${running(primary) ? 'Restart' : 'Run'} ${primary.name} (F5)\n${primary.command}`
              : 'No tasks found. Open the menu to add one.'
          }
        >
          {running(primary) ? (
            <span className="w-2 h-2 rounded-full bg-[var(--added-fg)] animate-pulse shrink-0" />
          ) : (
            <Play size={13} className="text-[var(--added-fg)] shrink-0" />
          )}
          <span className="hidden lg:inline truncate">{primary ? primary.name : 'Run'}</span>
        </button>
        {primary && running(primary) && (
          <>
            <button
              onClick={() => onRun(primary)}
              className="w-6 h-6 flex items-center justify-center rounded text-[var(--text-muted)] hover:bg-[var(--bg-active)] hover:text-[var(--text-primary)] cursor-pointer transition-colors"
              title="Restart"
              aria-label="Restart"
            >
              <RotateCw size={12} />
            </button>
            <button
              onClick={() => onStop(primary)}
              className="w-6 h-6 flex items-center justify-center rounded text-[var(--text-muted)] hover:bg-[var(--bg-active)] hover:text-[var(--danger)] cursor-pointer transition-colors"
              title="Stop (closes the task's terminal tab)"
              aria-label="Stop"
            >
              <Square size={12} />
            </button>
          </>
        )}
        <span className="w-px h-4 bg-[var(--border)] mx-0.5 shrink-0" aria-hidden />
        <button
          onClick={() => onOpenChange(!open)}
          className="w-6 h-6 flex items-center justify-center rounded text-[var(--text-muted)] hover:bg-[var(--bg-active)] hover:text-[var(--text-primary)] cursor-pointer transition-colors"
          title="Run a project task"
          aria-label="Open run menu"
        >
          <ChevronDown size={13} />
        </button>
      </div>

      {open && !draft && (
        <div className="absolute right-0 top-full mt-1.5 w-[360px] max-w-[90vw] rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-xl z-40 flex flex-col max-h-[70vh]">
          <div className="p-2 border-b border-[var(--border)]">
            <input
              ref={inputRef}
              className={inputCls}
              value={filter}
              placeholder="Filter, or type a command to run…"
              onChange={(e) => setFilter(e.target.value)}
              onKeyDown={onFilterKey}
            />
          </div>
          <div className="overflow-y-auto py-1">
            {flat.length === 0 ? (
              <div className="px-3 py-3 text-[12.5px] text-[var(--text-dim)]">
                {q ? (
                  <>
                    <span className="font-mono text-[var(--text-primary)]">{filter.trim()}</span>
                    <div className="mt-1">
                      Enter runs it in a terminal. Ctrl+Enter saves it as a custom command.
                    </div>
                  </>
                ) : (
                  'No tasks detected (package.json, Cargo.toml, pyproject, go.mod, Makefile, justfile, compose, scripts…). Add a custom command below.'
                )}
              </div>
            ) : (
              sections.map((s) => (
                <div key={s.key}>
                  <SectionLabel>{s.title}</SectionLabel>
                  {s.items.map(renderRow)}
                </div>
              ))
            )}
          </div>
          <div className="border-t border-[var(--border)] p-1">
            <button
              className="w-full flex items-center gap-2 px-2 py-1.5 rounded-md text-[12.5px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
              onClick={() => openForm(emptyDraft())}
            >
              <Plus size={13} />
              New custom command…
            </button>
          </div>
        </div>
      )}

      {open &&
        draft &&
        createPortal(
          <div
            data-run-modal
            className="fixed inset-0 z-50 flex items-center justify-center bg-black/55 backdrop-blur-[1px] p-4"
            onMouseDown={(e) => {
              if (e.target === e.currentTarget) setDraft(null);
            }}
          >
            <form
              className="w-full max-w-[480px] rounded-xl border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl overflow-hidden text-[var(--text-primary)]"
              onSubmit={(e) => {
                e.preventDefault();
                void submitDraft();
              }}
            >
              <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border)]">
                <div className="flex items-center gap-2 font-semibold text-sm">
                  <Terminal size={16} className="text-[var(--accent)]" />
                  {draft.id ? 'Edit custom command' : 'New custom command'}
                </div>
                <button
                  type="button"
                  onClick={() => setDraft(null)}
                  className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer"
                  aria-label="Close"
                >
                  <X size={15} />
                </button>
              </div>

              <div className="p-4 space-y-3.5 text-xs">
                <div className="space-y-1">
                  <label className={fieldLabel}>Name</label>
                  <input
                    className={fieldInput}
                    autoFocus
                    value={draft.name}
                    placeholder="e.g. Dev Server (Watch)"
                    onChange={(e) => setDraft({ ...draft, name: e.target.value })}
                  />
                </div>

                <div className="space-y-1.5">
                  <label className={fieldLabel}>Command</label>
                  <input
                    ref={commandRef}
                    className={`${fieldInput} font-mono`}
                    value={draft.command}
                    placeholder="cargo watch -x run"
                    spellCheck={false}
                    onChange={(e) => setDraft({ ...draft, command: e.target.value })}
                  />
                  <div className="flex flex-wrap items-center gap-1.5">
                    <span className="text-[11px] text-[var(--text-dim)]">Insert</span>
                    {COMMAND_VARS.map((v) => (
                      <button
                        key={v}
                        type="button"
                        onClick={() => insertVar(v)}
                        className="px-1.5 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-input)] font-mono text-[10.5px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:border-[var(--accent)] cursor-pointer"
                      >
                        {v}
                      </button>
                    ))}
                  </div>
                </div>

                <div className="space-y-1">
                  <label className={fieldLabel}>
                    Working directory <span className="font-normal text-[var(--text-dim)]">(optional)</span>
                  </label>
                  <input
                    className={`${fieldInput} font-mono`}
                    list="vireo-run-dirs"
                    value={draft.cwd}
                    placeholder="Workspace root"
                    spellCheck={false}
                    onChange={(e) => setDraft({ ...draft, cwd: e.target.value })}
                  />
                  <datalist id="vireo-run-dirs">
                    {workspaceDirs.map((d) => (
                      <option key={d} value={d} />
                    ))}
                  </datalist>
                </div>

                <div className="space-y-1">
                  <label className={fieldLabel}>Type</label>
                  <div className="flex p-0.5 gap-0.5 rounded-md border border-[var(--border)] bg-[var(--bg-input)]">
                    {CATEGORY_ORDER.map((c) => (
                      <button
                        key={c}
                        type="button"
                        onClick={() => setDraft({ ...draft, category: c })}
                        className={`flex-1 py-1 rounded text-[11.5px] capitalize cursor-pointer transition-colors ${
                          draft.category === c
                            ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                            : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                        }`}
                      >
                        {c}
                      </button>
                    ))}
                  </div>
                </div>

                <div className="space-y-1">
                  <label className={fieldLabel}>Save to</label>
                  <div className="grid grid-cols-2 gap-2">
                    {(
                      [
                        {
                          id: 'project',
                          title: 'Project',
                          desc: '.vireo/tasks.json, shareable',
                          icon: Users,
                        },
                        {
                          id: 'personal',
                          title: 'Personal',
                          desc: 'This machine only',
                          icon: Laptop,
                        },
                      ] as const
                    ).map((opt) => {
                      const active = draft.scope === opt.id;
                      const Icon = opt.icon;
                      return (
                        <button
                          key={opt.id}
                          type="button"
                          onClick={() => setDraft({ ...draft, scope: opt.id })}
                          className={`flex items-start gap-2 p-2.5 rounded-lg border text-left cursor-pointer transition-colors ${
                            active
                              ? 'border-[var(--accent)] bg-[var(--bg-active)]/50'
                              : 'border-[var(--border)] hover:bg-[var(--bg-hover)]'
                          }`}
                        >
                          <Icon
                            size={14}
                            className={`mt-0.5 shrink-0 ${active ? 'text-[var(--accent)]' : 'text-[var(--text-muted)]'}`}
                          />
                          <span className="min-w-0">
                            <span className="block font-medium text-[var(--text-primary)]">{opt.title}</span>
                            <span className="block text-[11px] text-[var(--text-muted)] mt-0.5 truncate">
                              {opt.desc}
                            </span>
                          </span>
                        </button>
                      );
                    })}
                  </div>
                </div>

                {formError && (
                  <div className="flex items-start gap-2 p-2.5 rounded-md bg-[var(--removed-bg)] border border-[var(--removed-fg)]/40 text-[11px] text-[var(--removed-fg)]">
                    <AlertTriangle size={14} className="shrink-0 mt-0.5" />
                    <span>{formError}</span>
                  </div>
                )}
              </div>

              <div className="flex justify-end gap-2 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)]/50">
                <button
                  type="button"
                  onClick={() => setDraft(null)}
                  className="px-3 py-1.5 text-xs rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer"
                >
                  Cancel
                </button>
                <button
                  type="submit"
                  disabled={saving || !draft.name.trim() || !draft.command.trim()}
                  className="px-3.5 py-1.5 text-xs font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:opacity-90 disabled:opacity-40 cursor-pointer"
                >
                  {saving ? 'Saving…' : 'Save'}
                </button>
              </div>
            </form>
          </div>,
          document.body
        )}
    </div>
  );
};
