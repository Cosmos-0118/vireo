import React, { useEffect, useMemo, useState } from 'react';
import { Keyboard, Lock, RotateCcw, Search, X } from 'lucide-react';
import {
  ACTIONS,
  FIXED_SHORTCUTS,
  GROUP_ORDER,
  bindingKeys,
  eventToBinding,
  findConflicts,
  formatBinding,
  getBinding,
  isCustomized,
  isValidBinding,
  resetAllBindings,
  resetBinding,
  setBinding,
  suspendShortcuts,
  useKeybindings,
  type ActionDef,
  type ActionId,
} from '../keybindings';

const Keys: React.FC<{ binding: string; dim?: boolean }> = ({ binding, dim }) => {
  const keys = bindingKeys(binding);
  if (keys.length === 0) return <span className="text-[11.5px] text-[var(--text-dim)] italic">Unbound</span>;
  return (
    <span className="inline-flex items-center gap-1">
      {keys.map((k, i) => (
        <kbd
          key={i}
          className={`inline-flex items-center justify-center min-w-[22px] h-[22px] px-1.5 rounded-[4px] border border-[var(--border)] bg-[var(--bg-elevated)] shadow-[inset_0_-1px_0_var(--border)] font-mono text-[11px] leading-none ${
            dim ? 'text-[var(--text-muted)]' : 'text-[var(--text-primary)]'
          }`}
        >
          {k}
        </kbd>
      ))}
    </span>
  );
};

const Group: React.FC<{ title: string; count: number; children: React.ReactNode }> = ({ title, count, children }) => (
  <section className="mb-5 break-inside-avoid overflow-hidden rounded-lg border border-[var(--border)] bg-[var(--bg-panel)]">
    <h3 className="flex items-center justify-between h-8 px-4 border-b border-[var(--border)] bg-[var(--bg-sidebar)] text-[10.5px] font-semibold uppercase tracking-[0.08em] text-[var(--text-dim)]">
      {title}
      <span className="font-mono font-normal tracking-normal tabular-nums">{count}</span>
    </h3>
    <div className="divide-y divide-[var(--border)]/60">{children}</div>
  </section>
);

const SectionHeading: React.FC<{ icon: React.ReactNode; title: string; note: string }> = ({ icon, title, note }) => (
  <div className="flex items-baseline gap-2 mb-3">
    <span className="self-center text-[var(--text-dim)]">{icon}</span>
    <h2 className="text-[12.5px] font-semibold text-[var(--text-primary)]">{title}</h2>
    <span className="min-w-0 truncate text-[11.5px] text-[var(--text-dim)]">{note}</span>
  </div>
);

// Groups flow into 1–3 balanced columns depending on the pane width (not the window).
const COLUMNS = 'columns-1 @[860px]:columns-2 @[1380px]:columns-3 gap-5';

const iconBtn =
  'w-6 h-6 inline-flex items-center justify-center rounded text-[var(--text-dim)] cursor-pointer hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] disabled:invisible';

export const ShortcutsPage: React.FC = () => {
  const overrides = useKeybindings(); // re-render on change
  const [query, setQuery] = useState('');
  const [recording, setRecording] = useState<ActionId | null>(null);
  const [hint, setHint] = useState<string | null>(null);
  const [pending, setPending] = useState<{ id: ActionId; binding: string; conflicts: ActionDef[] } | null>(null);

  // Capture the next key combo for the action being edited. Global shortcuts are suspended meanwhile.
  useEffect(() => {
    if (!recording) return;
    const release = suspendShortcuts();
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.repeat) return;
      if (e.key === 'Escape') {
        setRecording(null);
        setHint(null);
        return;
      }
      const b = eventToBinding(e);
      if (!b) return; // only modifiers so far
      if (!isValidBinding(b)) {
        setHint('Include Ctrl/Cmd or Alt, or use a function key (F1–F12).');
        return;
      }
      const conflicts = findConflicts(recording, b);
      setRecording(null);
      setHint(null);
      if (conflicts.length > 0) setPending({ id: recording, binding: b, conflicts });
      else setBinding(recording, b);
    };
    window.addEventListener('keydown', onKey, { capture: true });
    return () => {
      window.removeEventListener('keydown', onKey, { capture: true });
      release();
    };
  }, [recording]);

  const q = query.trim().toLowerCase();
  const matches = (label: string, binding: string) =>
    !q || label.toLowerCase().includes(q) || formatBinding(binding).toLowerCase().includes(q);

  const groups = useMemo(
    () =>
      GROUP_ORDER.map((g) => ({
        group: g,
        actions: ACTIONS.filter((a) => a.group === g && matches(a.label, getBinding(a.id))),
      })).filter((g) => g.actions.length > 0),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [q, overrides]
  );
  const fixed = FIXED_SHORTCUTS.map((g) => ({
    ...g,
    items: g.items.filter((i) => matches(i.label, i.keys)),
  })).filter((g) => g.items.length > 0);

  const customCount = Object.keys(overrides).length;
  const totalCount = ACTIONS.length + FIXED_SHORTCUTS.reduce((n, g) => n + g.items.length, 0);
  const nothing = groups.length === 0 && fixed.length === 0;

  const applyPending = () => {
    if (!pending) return;
    for (const c of pending.conflicts) {
      // Only unbind actions whose *editable* binding collides; built-in alternates stay as they are.
      if (getBinding(c.id) === pending.binding) setBinding(c.id, '');
    }
    setBinding(pending.id, pending.binding);
    setPending(null);
  };

  return (
    <div className="@container flex-1 min-h-0 overflow-y-auto bg-[var(--bg-base)]">
      {/* Sticky header: title, stats, search and reset stay reachable while scrolling */}
      <div className="sticky top-0 z-[2] border-b border-[var(--border)] bg-[var(--bg-base)]/95 backdrop-blur-sm">
        <div className="mx-auto w-full max-w-[1680px] px-6 @[1200px]:px-10 pt-6 pb-4">
          <header className="flex flex-wrap items-end justify-between gap-x-6 gap-y-3">
            <div className="min-w-0">
              <h1 className="flex items-center gap-2 text-[17px] font-semibold tracking-tight text-[var(--text-primary)]">
                <Keyboard size={17} className="text-[var(--accent)]" />
                Keyboard Shortcuts
              </h1>
              <p className="mt-1 text-[12px] text-[var(--text-dim)]">
                Click a shortcut to change it. Esc cancels, <X size={10} className="inline -mt-px" /> unbinds,{' '}
                <RotateCcw size={10} className="inline -mt-px" /> restores the default.
              </p>
            </div>
            <div className="flex items-center gap-2 w-full @[700px]:w-auto">
              <div className="relative flex-1 @[700px]:w-[340px] @[700px]:flex-none">
                <Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-dim)]" />
                <input
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="Search by action or key (e.g. terminal, Ctrl+B)"
                  className="w-full h-8 pl-8 pr-7 rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[12.5px] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none focus:border-[var(--accent)]"
                />
                {query && (
                  <button
                    type="button"
                    onClick={() => setQuery('')}
                    aria-label="Clear search"
                    className="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 rounded text-[var(--text-dim)] hover:text-[var(--text-primary)] cursor-pointer"
                  >
                    <X size={12} />
                  </button>
                )}
              </div>
              <button
                type="button"
                onClick={resetAllBindings}
                disabled={customCount === 0}
                className="shrink-0 inline-flex items-center gap-1.5 h-8 px-3 rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] text-[12px] text-[var(--text-muted)] cursor-pointer hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] disabled:opacity-40 disabled:cursor-default disabled:hover:bg-[var(--bg-elevated)]"
              >
                <RotateCcw size={12} />
                Reset all{customCount > 0 ? ` (${customCount})` : ''}
              </button>
            </div>
          </header>
          <div className="mt-3 flex items-center gap-3 text-[11px] text-[var(--text-dim)] tabular-nums">
            <span>{totalCount} shortcuts</span>
            <span className="w-px h-3 bg-[var(--border)]" />
            <span>{ACTIONS.length} customizable</span>
            <span className="w-px h-3 bg-[var(--border)]" />
            <span className={customCount > 0 ? 'text-[var(--accent)]' : ''}>{customCount} changed</span>
          </div>
        </div>
      </div>

      <div className="mx-auto w-full max-w-[1680px] px-6 @[1200px]:px-10 py-6">
        {groups.length > 0 && (
          <SectionHeading icon={<Keyboard size={13} />} title="Customizable" note="Your bindings, saved across restarts." />
        )}
        <div className={COLUMNS}>
          {groups.map(({ group, actions }) => (
            <Group key={group} title={group} count={actions.length}>
              {actions.map((a) => {
                const binding = getBinding(a.id);
                const isRec = recording === a.id;
                const custom = isCustomized(a.id);
                const isPending = pending?.id === a.id;
                return (
                  <div
                    key={a.id}
                    className={`group/row relative px-4 py-2 transition-colors hover:bg-[var(--bg-hover)]/50 ${
                      isRec ? 'bg-[color-mix(in_srgb,var(--accent)_6%,transparent)]' : ''
                    }`}
                  >
                    {custom && <span className="absolute left-0 top-2 bottom-2 w-[2px] rounded-r bg-[var(--accent)]" title="Customized" />}
                    <div className="flex items-center justify-between gap-3 min-h-[28px]">
                      <div className="min-w-0">
                        <div className="text-[13px] text-[var(--text-primary)] truncate" title={a.label}>
                          {a.label}
                        </div>
                        {a.alts && (
                          <div className="mt-0.5 text-[11px] text-[var(--text-dim)] truncate" title="Always-on alternates">
                            also {a.alts.map((x) => formatBinding(x)).join(', ')}
                          </div>
                        )}
                      </div>
                      <div className="flex items-center gap-1 shrink-0">
                        <button
                          type="button"
                          onClick={() => {
                            setPending(null);
                            setHint(null);
                            setRecording(isRec ? null : a.id);
                          }}
                          title={isRec ? 'Press the new shortcut (Esc to cancel)' : 'Click to change'}
                          className={`min-w-[132px] h-7 px-2 inline-flex items-center justify-end rounded-md border cursor-pointer transition-colors ${
                            isRec
                              ? 'justify-center border-[var(--accent)] bg-[color-mix(in_srgb,var(--accent)_12%,transparent)] text-[12px] text-[var(--accent)]'
                              : 'border-transparent hover:border-[var(--border)] hover:bg-[var(--bg-elevated)]'
                          }`}
                        >
                          {isRec ? 'Press keys…' : <Keys binding={binding} />}
                        </button>
                        <button
                          type="button"
                          onClick={() => setBinding(a.id, '')}
                          disabled={!binding}
                          title="Unbind"
                          aria-label={`Unbind ${a.label}`}
                          className={`${iconBtn} opacity-0 group-hover/row:opacity-100 focus-visible:opacity-100`}
                        >
                          <X size={12} />
                        </button>
                        <button
                          type="button"
                          onClick={() => resetBinding(a.id)}
                          disabled={!custom}
                          title={`Reset to default (${formatBinding(a.default) || 'unbound'})`}
                          aria-label={`Reset ${a.label}`}
                          className={iconBtn}
                        >
                          <RotateCcw size={12} />
                        </button>
                      </div>
                    </div>
                    {isRec && hint && <div className="mt-1.5 text-[11.5px] text-[var(--modified-fg)]">{hint}</div>}
                    {isPending && pending && (
                      <div className="mt-2 flex flex-wrap items-center justify-between gap-x-3 gap-y-2 rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] px-3 py-2 text-[12px] text-[var(--text-muted)]">
                        <span className="min-w-0">
                          <b className="font-mono text-[var(--text-primary)]">{formatBinding(pending.binding)}</b> is already
                          used by {pending.conflicts.map((c) => `“${c.label}”`).join(', ')}.
                        </span>
                        <span className="flex items-center gap-1.5 shrink-0">
                          <button
                            type="button"
                            onClick={() => setPending(null)}
                            className="h-6 px-2 rounded border border-[var(--border)] cursor-pointer hover:bg-[var(--bg-hover)]"
                          >
                            Cancel
                          </button>
                          <button
                            type="button"
                            onClick={applyPending}
                            className="h-6 px-2 rounded bg-[var(--accent)] text-[var(--accent-fg)] cursor-pointer hover:opacity-90"
                          >
                            Reassign
                          </button>
                        </span>
                      </div>
                    )}
                  </div>
                );
              })}
            </Group>
          ))}
        </div>

        {fixed.length > 0 && (
          <div className={groups.length > 0 ? 'mt-6' : ''}>
            <SectionHeading
              icon={<Lock size={12} />}
              title="Built-in"
              note="Owned by the editor, terminal and dialogs; can’t be changed here."
            />
            <div className={COLUMNS}>
              {fixed.map((g) => (
                <Group key={g.group} title={g.group} count={g.items.length}>
                  {g.items.map((i) => (
                    <div
                      key={i.keys + i.label}
                      className="flex items-center justify-between gap-3 min-h-[44px] px-4 py-2 hover:bg-[var(--bg-hover)]/50"
                    >
                      <div className="min-w-0 text-[13px] text-[var(--text-muted)] truncate" title={i.label}>
                        {i.label}
                      </div>
                      <span className="shrink-0">
                        <Keys binding={i.keys} dim />
                      </span>
                    </div>
                  ))}
                </Group>
              ))}
            </div>
          </div>
        )}

        {nothing && (
          <div className="py-16 text-center text-[12.5px] text-[var(--text-dim)]">No shortcuts match “{query}”.</div>
        )}
      </div>
    </div>
  );
};
