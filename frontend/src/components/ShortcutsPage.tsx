import React, { useEffect, useMemo, useState } from 'react';
import { RotateCcw, Search, X } from 'lucide-react';
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
          className={`min-w-[22px] px-1.5 py-[2px] rounded border border-[var(--border)] border-b-2 bg-[var(--bg-elevated)] text-center font-mono text-[11px] leading-none ${
            dim ? 'text-[var(--text-muted)]' : 'text-[var(--text-primary)]'
          }`}
        >
          {k}
        </kbd>
      ))}
    </span>
  );
};

const Group: React.FC<{ title: string; children: React.ReactNode }> = ({ title, children }) => (
  <section className="mb-6 last:mb-0">
    <h2 className="mb-2 px-0.5 text-[11px] font-medium uppercase tracking-wider text-[var(--text-dim)]">{title}</h2>
    <div className="rounded-lg border border-[var(--border)] bg-[var(--bg-panel)] divide-y divide-[var(--border)]/70">
      {children}
    </div>
  </section>
);

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
    <div className="flex-1 min-h-0 overflow-y-auto bg-[var(--bg-base)]">
      <div className="mx-auto w-full max-w-[720px] px-6 py-8">
        <header className="flex items-center justify-between gap-4 mb-4">
          <div>
            <h1 className="text-[16px] font-semibold tracking-tight text-[var(--text-primary)]">Keyboard Shortcuts</h1>
            <p className="mt-1 text-[12px] text-[var(--text-dim)]">
              Click a shortcut to change it. Esc cancels, the cross unbinds, the arrow restores the default.
            </p>
          </div>
          <button
            type="button"
            onClick={resetAllBindings}
            disabled={customCount === 0}
            className="shrink-0 inline-flex items-center gap-1.5 h-7 px-2.5 rounded-md border border-[var(--border)] text-[12px] text-[var(--text-muted)] cursor-pointer hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] disabled:opacity-40 disabled:cursor-default disabled:hover:bg-transparent"
          >
            <RotateCcw size={12} />
            Reset all{customCount > 0 ? ` (${customCount})` : ''}
          </button>
        </header>

        <div className="relative mb-6">
          <Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-dim)]" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search by action or key (e.g. terminal, Ctrl+B)"
            className="w-full h-8 pl-8 pr-2.5 rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[12.5px] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none focus:border-[var(--accent)]"
          />
        </div>

        {groups.map(({ group, actions }) => (
          <Group key={group} title={group}>
            {actions.map((a) => {
              const binding = getBinding(a.id);
              const isRec = recording === a.id;
              const custom = isCustomized(a.id);
              const isPending = pending?.id === a.id;
              return (
                <div key={a.id} className="px-4 py-2.5">
                  <div className="flex items-center justify-between gap-4">
                    <div className="min-w-0 text-[13px] text-[var(--text-primary)] truncate">{a.label}</div>
                    <div className="flex items-center gap-2 shrink-0">
                      {a.alts && !isRec && (
                        <span className="hidden sm:inline text-[11px] text-[var(--text-dim)]" title="Always-on alternates">
                          also {a.alts.map((x) => formatBinding(x)).join(', ')}
                        </span>
                      )}
                      <button
                        type="button"
                        onClick={() => {
                          setPending(null);
                          setHint(null);
                          setRecording(isRec ? null : a.id);
                        }}
                        title={isRec ? 'Press the new shortcut (Esc to cancel)' : 'Click to change'}
                        className={`min-w-[120px] h-7 px-2 inline-flex items-center justify-center rounded-md border cursor-pointer transition-colors ${
                          isRec
                            ? 'border-[var(--accent)] bg-[color-mix(in_srgb,var(--accent)_12%,transparent)] text-[12px] text-[var(--accent)]'
                            : 'border-transparent hover:border-[var(--border)] hover:bg-[var(--bg-hover)]'
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
                        className="w-6 h-6 inline-flex items-center justify-center rounded text-[var(--text-dim)] cursor-pointer hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] disabled:opacity-0 disabled:cursor-default"
                      >
                        <X size={12} />
                      </button>
                      <button
                        type="button"
                        onClick={() => resetBinding(a.id)}
                        disabled={!custom}
                        title={`Reset to default (${formatBinding(a.default) || 'unbound'})`}
                        aria-label={`Reset ${a.label}`}
                        className="w-6 h-6 inline-flex items-center justify-center rounded text-[var(--text-dim)] cursor-pointer hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] disabled:opacity-0 disabled:cursor-default"
                      >
                        <RotateCcw size={12} />
                      </button>
                    </div>
                  </div>
                  {isRec && hint && <div className="mt-1.5 text-[11.5px] text-[var(--modified-fg)]">{hint}</div>}
                  {isPending && pending && (
                    <div className="mt-2 flex items-center justify-between gap-3 rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] px-3 py-2 text-[12px] text-[var(--text-muted)]">
                      <span>
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

        {fixed.length > 0 && (
          <div className="mt-8 mb-3 px-0.5 text-[11.5px] text-[var(--text-dim)]">
            Built-in shortcuts below are owned by the editor, terminal and dialogs and can’t be changed here.
          </div>
        )}
        {fixed.map((g) => (
          <Group key={g.group} title={g.group}>
            {g.items.map((i) => (
              <div key={i.keys + i.label} className="flex items-center justify-between gap-4 px-4 py-2.5">
                <div className="min-w-0 text-[13px] text-[var(--text-muted)] truncate">{i.label}</div>
                <Keys binding={i.keys} dim />
              </div>
            ))}
          </Group>
        ))}

        {nothing && (
          <div className="py-12 text-center text-[12.5px] text-[var(--text-dim)]">No shortcuts match “{query}”.</div>
        )}
      </div>
    </div>
  );
};
