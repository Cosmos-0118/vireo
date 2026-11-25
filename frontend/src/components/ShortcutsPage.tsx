import React, { useEffect, useMemo, useState } from 'react';
import { Lock, RotateCcw, Search, X } from 'lucide-react';
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

const iconBtn =
  'w-6 h-6 inline-flex items-center justify-center rounded text-[var(--text-dim)] cursor-pointer hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] disabled:invisible';

/** Score one query token against text: substring (word-start best) beats an in-order subsequence; -1 = no match. */
function tokenScore(token: string, text: string): number {
  const at = text.indexOf(token);
  if (at >= 0) return 1000 - at + (at === 0 || /[\s+]/.test(text[at - 1]) ? 200 : 0);
  let ti = 0;
  let run = 0;
  let score = 0;
  let prev = -2;
  for (let i = 0; i < text.length && ti < token.length; i++) {
    if (text[i] !== token[ti]) continue;
    run = i === prev + 1 ? run + 1 : 0;
    score += 10 + run * 8 + (i === 0 || /[\s+]/.test(text[i - 1]) ? 15 : 0);
    prev = i;
    ti++;
  }
  return ti === token.length ? score : -1;
}

/** Every whitespace-separated token must match; higher = better. */
function fuzzyScore(query: string, text: string): number {
  let total = 0;
  for (const t of query.split(/\s+/).filter(Boolean)) {
    const s = tokenScore(t, text);
    if (s < 0) return -1;
    total += s;
  }
  return total;
}

type Row =
  | { kind: 'action'; group: string; action: ActionDef; hay: string }
  | { kind: 'fixed'; group: string; label: string; keys: string; hay: string };

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

  // One linear list: customizable groups first, then built-ins, each group in order.
  const rows = useMemo<Row[]>(() => {
    const out: Row[] = [];
    for (const g of GROUP_ORDER) {
      for (const a of ACTIONS.filter((x) => x.group === g)) {
        const keys = [getBinding(a.id), ...(a.alts ?? [])].map((k) => formatBinding(k)).join(' ');
        out.push({ kind: 'action', group: g, action: a, hay: `${a.label} ${g} ${keys}`.toLowerCase() });
      }
    }
    for (const g of FIXED_SHORTCUTS) {
      for (const i of g.items) {
        out.push({ kind: 'fixed', group: g.group, label: i.label, keys: i.keys, hay: `${i.label} ${g.group} ${formatBinding(i.keys)}`.toLowerCase() });
      }
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [overrides]);

  const q = query.trim().toLowerCase();
  const visible = useMemo(() => {
    if (!q) return rows;
    return rows
      .map((r) => ({ r, s: fuzzyScore(q, r.hay) }))
      .filter((x) => x.s >= 0)
      .sort((a, b) => b.s - a.s)
      .map((x) => x.r);
  }, [rows, q]);

  const customCount = Object.keys(overrides).length;

  const applyPending = () => {
    if (!pending) return;
    for (const c of pending.conflicts) {
      // Only unbind actions whose *editable* binding collides; built-in alternates stay as they are.
      if (getBinding(c.id) === pending.binding) setBinding(c.id, '');
    }
    setBinding(pending.id, pending.binding);
    setPending(null);
  };

  const renderRow = (r: Row, showGroup: boolean) => {
    if (r.kind === 'fixed') {
      return (
        <div key={`f:${r.group}:${r.keys}:${r.label}`} className="flex items-center justify-between gap-4 min-h-[38px] px-3 hover:bg-[var(--bg-hover)]/50">
          <div className="min-w-0 flex items-center gap-2 text-[13px] text-[var(--text-muted)]">
            <span className="truncate" title={r.label}>{r.label}</span>
            {showGroup && <span className="shrink-0 text-[11px] text-[var(--text-dim)]">{r.group}</span>}
          </div>
          <span className="shrink-0 inline-flex items-center gap-2" title="Built-in: can't be changed">
            <Lock size={11} className="text-[var(--text-dim)]" />
            <Keys binding={r.keys} dim />
          </span>
        </div>
      );
    }
    const a = r.action;
    const binding = getBinding(a.id);
    const isRec = recording === a.id;
    const custom = isCustomized(a.id);
    const isPending = pending?.id === a.id;
    return (
      <div
        key={a.id}
        className={`group/row relative px-3 transition-colors hover:bg-[var(--bg-hover)]/50 ${isRec ? 'bg-[color-mix(in_srgb,var(--accent)_6%,transparent)]' : ''}`}
      >
        {custom && <span className="absolute left-0 top-2 bottom-2 w-[2px] rounded-r bg-[var(--accent)]" title="Customized" />}
        <div className="flex items-center justify-between gap-4 min-h-[38px]">
          <div className="min-w-0">
            <div className="flex items-center gap-2 text-[13px] text-[var(--text-primary)]">
              <span className="truncate" title={a.label}>{a.label}</span>
              {showGroup && <span className="shrink-0 text-[11px] text-[var(--text-dim)]">{r.group}</span>}
            </div>
            {a.alts && (
              <div className="text-[11px] text-[var(--text-dim)] truncate" title="Always-on alternates">
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
        {isRec && hint && <div className="pb-2 text-[11.5px] text-[var(--modified-fg)]">{hint}</div>}
        {isPending && pending && (
          <div className="mb-2 flex flex-wrap items-center justify-between gap-x-3 gap-y-2 rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] px-3 py-2 text-[12px] text-[var(--text-muted)]">
            <span className="min-w-0">
              <b className="font-mono text-[var(--text-primary)]">{formatBinding(pending.binding)}</b> is already used by{' '}
              {pending.conflicts.map((c) => `“${c.label}”`).join(', ')}.
            </span>
            <span className="flex items-center gap-1.5 shrink-0">
              <button type="button" onClick={() => setPending(null)} className="h-6 px-2 rounded border border-[var(--border)] cursor-pointer hover:bg-[var(--bg-hover)]">
                Cancel
              </button>
              <button type="button" onClick={applyPending} className="h-6 px-2 rounded bg-[var(--accent)] text-[var(--accent-fg)] cursor-pointer hover:opacity-90">
                Reassign
              </button>
            </span>
          </div>
        )}
      </div>
    );
  };

  // Group headings only when browsing (not while a search ranks results across groups).
  const body: React.ReactNode[] = [];
  let last = '';
  for (const r of visible) {
    const gKey = `${r.kind}:${r.group}`;
    if (!q && gKey !== last) {
      last = gKey;
      body.push(
        <h3 key={`h:${gKey}`} className="px-3 pt-5 pb-1 text-[11px] font-semibold uppercase tracking-[0.08em] text-[var(--text-dim)] first:pt-1">
          {r.group}
          {r.kind === 'fixed' && <span className="ml-1.5 normal-case tracking-normal font-normal">· built-in</span>}
        </h3>
      );
    }
    body.push(renderRow(r, !!q));
  }

  return (
    <div className="flex-1 min-h-0 overflow-y-auto bg-[var(--bg-base)]">
      <div className="sticky top-0 z-[2] bg-[var(--bg-base)]/95 backdrop-blur-sm border-b border-[var(--border)]">
        <div className="mx-auto w-full max-w-[920px] px-6 py-3 flex items-center gap-2">
          <div className="relative flex-1">
            <Search size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--text-dim)]" />
            <input
              autoFocus
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search shortcuts…"
              spellCheck={false}
              className="w-full h-9 pl-9 pr-8 rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[13px] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none focus:border-[var(--accent)]"
            />
            {query && (
              <button
                type="button"
                onClick={() => setQuery('')}
                aria-label="Clear search"
                className="absolute right-2 top-1/2 -translate-y-1/2 p-0.5 rounded text-[var(--text-dim)] hover:text-[var(--text-primary)] cursor-pointer"
              >
                <X size={13} />
              </button>
            )}
          </div>
          {customCount > 0 && (
            <button
              type="button"
              onClick={resetAllBindings}
              className="shrink-0 inline-flex items-center gap-1.5 h-9 px-3 rounded-md border border-[var(--border)] text-[12px] text-[var(--text-muted)] cursor-pointer hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]"
            >
              <RotateCcw size={12} />
              Reset all ({customCount})
            </button>
          )}
        </div>
      </div>

      <div className="mx-auto w-full max-w-[920px] px-6 py-4 pb-10">
        {body}
        {visible.length === 0 && (
          <div className="py-16 text-center text-[12.5px] text-[var(--text-dim)]">No shortcuts match “{query}”.</div>
        )}
      </div>
    </div>
  );
};
