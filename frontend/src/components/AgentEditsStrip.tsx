import { MagnifyingGlass } from '@phosphor-icons/react';
import { EmptyRow } from './ui/EmptyState';
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { ChevronDown, ChevronLeft, ChevronRight, Search } from 'lucide-react';
import { FileIcon } from './FileIcon';

export interface AgentEdit {
  path: string;
  adds: number;
  dels: number;
  at: number;
}

interface Props {
  /** Edits in arrival order (oldest first); shown newest first. */
  edits: AgentEdit[];
  activePath: string | null;
  tabPulse: Record<string, number>;
  onFocus: (path: string) => void;
}

const CHIP_GAP = 2; // gap-0.5
const MORE_WIDTH = 44; // room reserved for the "+N" button
const FILTER_THRESHOLD = 8;

const baseName = (p: string) => p.split('/').pop() || p;
const dirName = (p: string) => {
  const i = p.lastIndexOf('/');
  return i > 0 ? p.slice(0, i) : '';
};

/**
 * Left side of the agent-edits bar: "N files" picker, prev/next stepper and as many
 * file chips as fit. Files that don't fit collapse into a "+N" button; both it and the
 * "N files" label open a searchable list of every edited file — no horizontal scrolling.
 */
export function AgentEditsStrip({ edits, activePath, tabPulse, onFocus }: Props) {
  const items = useMemo(() => edits.slice().reverse(), [edits]);
  const n = items.length;
  const activeIdx = activePath ? items.findIndex((e) => e.path === activePath) : -1;

  // --- fit calculation (measured off-screen, so what we render never feeds back) ---
  const stripRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLDivElement>(null);
  const [widths, setWidths] = useState<number[]>([]);
  const [avail, setAvail] = useState(0);

  useLayoutEffect(() => {
    const m = measureRef.current;
    if (!m) return;
    setWidths(Array.from(m.children, (c) => (c as HTMLElement).offsetWidth));
  }, [items]);

  useLayoutEffect(() => {
    const el = stripRef.current;
    if (!el) return;
    const update = () => setAvail(el.clientWidth);
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const visible = useMemo<number[]>(() => {
    if (!n || widths.length !== n || avail <= 0) return items.map((_, i) => i);
    const total = widths.reduce((s, w) => s + w, 0) + CHIP_GAP * (n - 1);
    if (total <= avail) return items.map((_, i) => i);
    const room = avail - MORE_WIDTH - CHIP_GAP;
    let k = 0;
    let used = 0;
    while (k < n && used + widths[k] + (k ? CHIP_GAP : 0) <= room) {
      used += widths[k] + (k ? CHIP_GAP : 0);
      k++;
    }
    let idx = Array.from({ length: k }, (_, i) => i);
    // Keep the file you're looking at on screen even when it's in the overflow.
    if (activeIdx >= k && k > 0) {
      while (idx.length) {
        const cand = [...idx.slice(0, -1), activeIdx];
        const w = cand.reduce((s, i) => s + widths[i], 0) + CHIP_GAP * (cand.length - 1);
        if (w <= room) {
          idx = cand;
          break;
        }
        idx = idx.slice(0, -1);
      }
    }
    return idx;
  }, [items, n, widths, avail, activeIdx]);

  const hiddenCount = n - visible.length;

  // --- stepper ---
  const step = (dir: 1 | -1) => {
    if (!n) return;
    const next = activeIdx < 0 ? (dir === 1 ? 0 : n - 1) : (activeIdx + dir + n) % n;
    onFocus(items[next].path);
  };

  // --- popover ---
  const [open, setOpen] = useState(false);
  const anchorRef = useRef<HTMLButtonElement>(null);

  const chip = (e: AgentEdit, measuring = false) => {
    const current = !measuring && activePath === e.path;
    const pulse = !measuring && tabPulse[e.path] !== undefined ? `row-pulse-${tabPulse[e.path] % 2}` : '';
    return (
      <button
        key={e.path}
        tabIndex={measuring ? -1 : undefined}
        onClick={measuring ? undefined : () => onFocus(e.path)}
        title={e.path}
        className={`h-6 px-2 rounded-md flex items-center gap-1.5 shrink-0 cursor-pointer transition-colors ${
          current
            ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
            : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
        } ${pulse}`}
      >
        <FileIcon path={e.path} size={12} />
        <span className="whitespace-nowrap">{baseName(e.path)}</span>
        <span className="font-mono text-[11px] tabular-nums text-[var(--added-fg)]">+{e.adds}</span>
        <span className="font-mono text-[11px] tabular-nums text-[var(--removed-fg)]">-{e.dels}</span>
      </button>
    );
  };

  return (
    <>
      <div className="flex items-center gap-1 shrink-0">
        <span className="live-dot w-1.5 h-1.5 rounded-full bg-[var(--added-fg)] mr-1" />
        <span className="font-medium text-[var(--text-primary)]">Agent edits</span>
        <button
          ref={anchorRef}
          onClick={() => setOpen((v) => !v)}
          aria-expanded={open}
          className={`h-6 pl-1.5 pr-1 rounded-md flex items-center gap-0.5 tabular-nums cursor-pointer transition-colors ${
            open
              ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
              : 'text-[var(--text-dim)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
          }`}
          title="Show all edited files"
        >
          {n} {n === 1 ? 'file' : 'files'}
          <ChevronDown size={12} />
        </button>
        {n > 1 && (
          <div className="flex items-center">
            <button
              onClick={() => step(-1)}
              className="h-6 w-5 rounded-md flex items-center justify-center text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] cursor-pointer"
              title="Previous edited file"
            >
              <ChevronLeft size={13} />
            </button>
            <button
              onClick={() => step(1)}
              className="h-6 w-5 rounded-md flex items-center justify-center text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] cursor-pointer"
              title="Next edited file"
            >
              <ChevronRight size={13} />
            </button>
          </div>
        )}
      </div>
      <span className="w-px h-4 bg-[var(--border)] shrink-0" />
      <div ref={stripRef} className="relative flex items-center gap-0.5 min-w-0 flex-1 overflow-hidden">
        {visible.map((i) => chip(items[i]))}
        {hiddenCount > 0 && (
          <button
            onClick={() => setOpen((v) => !v)}
            className="h-6 px-2 rounded-md shrink-0 tabular-nums text-[var(--text-muted)] border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] cursor-pointer"
            data-edits-trigger
            title={`${hiddenCount} more edited ${hiddenCount === 1 ? 'file' : 'files'}`}
          >
            +{hiddenCount}
          </button>
        )}
        {/* Off-screen measurer: natural width of every chip. */}
        <div
          ref={measureRef}
          aria-hidden
          className="absolute left-0 top-0 w-max flex gap-0.5 invisible pointer-events-none"
        >
          {items.map((e) => chip(e, true))}
        </div>
      </div>
      {open && (
        <EditsPopover
          items={items}
          activePath={activePath}
          tabPulse={tabPulse}
          anchor={anchorRef.current}
          onPick={(p) => {
            setOpen(false);
            onFocus(p);
          }}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}

function EditsPopover({
  items,
  activePath,
  tabPulse,
  anchor,
  onPick,
  onClose,
}: {
  items: AgentEdit[];
  activePath: string | null;
  tabPulse: Record<string, number>;
  anchor: HTMLElement | null;
  onPick: (path: string) => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState('');
  const showFilter = items.length > FILTER_THRESHOLD;

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q ? items.filter((e) => e.path.toLowerCase().includes(q)) : items;
  }, [items, query]);

  const [sel, setSel] = useState(() => Math.max(0, items.findIndex((e) => e.path === activePath)));
  useEffect(() => setSel((s) => Math.min(s, Math.max(0, filtered.length - 1))), [filtered.length]);
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-i="${sel}"]`)?.scrollIntoView({ block: 'nearest' });
  }, [sel]);

  const totals = useMemo(
    () => items.reduce((t, e) => ({ adds: t.adds + e.adds, dels: t.dels + e.dels }), { adds: 0, dels: 0 }),
    [items]
  );

  // Position above the anchor (the bar is docked at the bottom of the window).
  const [pos, setPos] = useState<{ left: number; bottom: number } | null>(null);
  useLayoutEffect(() => {
    const place = () => {
      if (!anchor) return;
      const r = anchor.getBoundingClientRect();
      const width = Math.min(420, window.innerWidth - 16);
      setPos({
        left: Math.max(8, Math.min(r.left - 4, window.innerWidth - width - 8)),
        bottom: window.innerHeight - r.top + 6,
      });
    };
    place();
    window.addEventListener('resize', place);
    return () => window.removeEventListener('resize', place);
  }, [anchor]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (ref.current?.contains(t) || anchor?.contains(t)) return;
      if ((t as Element).closest?.("[data-edits-trigger]")) return;
      onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onClose();
      }
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    window.addEventListener('blur', onClose);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
      window.removeEventListener('blur', onClose);
    };
  }, [anchor, onClose]);

  useEffect(() => {
    // Focus the popover so arrow keys work without a filter box.
    if (!showFilter) ref.current?.focus();
  }, [showFilter]);

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setSel((s) => Math.min(filtered.length - 1, s + 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setSel((s) => Math.max(0, s - 1));
    } else if (e.key === 'Enter' && filtered[sel]) {
      e.preventDefault();
      onPick(filtered[sel].path);
    }
  };

  if (!pos) return null;

  return createPortal(
    <div
      ref={ref}
      tabIndex={-1}
      onKeyDown={onKeyDown}
      style={{ left: pos.left, bottom: pos.bottom, width: Math.min(420, window.innerWidth - 16) }}
      className="fixed z-[100] rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-[0_16px_40px_-10px_rgba(0,0,0,0.65),0_2px_8px_rgba(0,0,0,0.3)] text-xs outline-none flex flex-col overflow-hidden"
    >
      <div className="flex items-center gap-2 px-3 h-8 border-b border-[var(--border)] shrink-0">
        <span className="font-medium text-[var(--text-primary)]">
          {items.length} {items.length === 1 ? 'file' : 'files'} changed
        </span>
        <span className="ml-auto font-mono text-[11px] tabular-nums text-[var(--added-fg)]">+{totals.adds}</span>
        <span className="font-mono text-[11px] tabular-nums text-[var(--removed-fg)]">-{totals.dels}</span>
      </div>
      {showFilter && (
        <div className="flex items-center gap-2 px-3 h-8 border-b border-[var(--border)] shrink-0">
          <Search size={12} className="text-[var(--text-dim)] shrink-0" />
          <input
            autoFocus
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setSel(0);
            }}
            placeholder="Filter files…"
            className="flex-1 min-w-0 bg-transparent outline-none text-[var(--text-primary)] placeholder:text-[var(--text-dim)]"
          />
        </div>
      )}
      <div ref={listRef} className="overflow-y-auto py-1" style={{ maxHeight: 'min(50vh, 360px)' }}>
        {filtered.length === 0 && <EmptyRow icon={MagnifyingGlass}>No matching files</EmptyRow>}
        {filtered.map((e, i) => {
          const current = activePath === e.path;
          const dir = dirName(e.path);
          const pulse = tabPulse[e.path] !== undefined ? `row-pulse-${tabPulse[e.path] % 2}` : '';
          return (
            <button
              key={e.path}
              data-i={i}
              onClick={() => onPick(e.path)}
              onMouseMove={() => sel !== i && setSel(i)}
              title={e.path}
              className={`w-full h-7 px-3 flex items-center gap-2 text-left cursor-pointer ${
                i === sel ? 'bg-[var(--bg-hover)]' : ''
              } ${pulse}`}
            >
              <FileIcon path={e.path} size={13} />
              <span
                className={`shrink-0 ${current ? 'text-[var(--accent)] font-medium' : 'text-[var(--text-primary)]'}`}
              >
                {baseName(e.path)}
              </span>
              <span className="min-w-0 flex-1 truncate text-[var(--text-dim)]">{dir}</span>
              <span className="font-mono text-[11px] tabular-nums text-[var(--added-fg)] shrink-0">+{e.adds}</span>
              <span className="font-mono text-[11px] tabular-nums text-[var(--removed-fg)] shrink-0 w-8 text-right">
                -{e.dels}
              </span>
            </button>
          );
        })}
      </div>
    </div>,
    document.body
  );
}
