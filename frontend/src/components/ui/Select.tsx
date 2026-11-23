import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown } from 'lucide-react';

export interface SelectOption {
  value: string;
  label: string;
  hint?: string;
  icon?: React.ReactNode;
  group?: string;
  disabled?: boolean;
  /** Small trailing pill, e.g. "Installed" or "Not installed". */
  badge?: string;
  /** Visual tone for the trailing badge. */
  badgeTone?: 'ok' | 'warn' | 'muted';
  /** Optional status indicator dot before the icon/label. */
  dot?: 'ok' | 'warn' | 'muted';
}

interface Pos {
  left: number;
  width: number;
  top?: number;
  bottom?: number;
  maxHeight: number;
}

const MENU_MAX = 300;

/** Positions a portalled popover under (or above) an anchor and closes it on outside interaction. */
function usePopover(open: boolean, onClose: () => void, anchor: React.RefObject<HTMLElement | null>) {
  const menuRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<Pos | null>(null);

  const place = useCallback(() => {
    const el = anchor.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const below = window.innerHeight - r.bottom - 12;
    const above = r.top - 12;
    const flip = below < 180 && above > below;
    const maxHeight = Math.max(140, Math.min(MENU_MAX, flip ? above : below));
    setPos({
      left: r.left,
      width: r.width,
      maxHeight,
      ...(flip ? { bottom: window.innerHeight - r.top + 4 } : { top: r.bottom + 4 }),
    });
  }, [anchor]);

  useLayoutEffect(() => {
    if (open) place();
  }, [open, place]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (menuRef.current?.contains(t) || anchor.current?.contains(t)) return;
      onClose();
    };
    const onScroll = (e: Event) => {
      if (menuRef.current?.contains(e.target as Node)) return;
      place();
    };
    document.addEventListener('mousedown', onDown);
    window.addEventListener('resize', place);
    window.addEventListener('scroll', onScroll, true);
    return () => {
      document.removeEventListener('mousedown', onDown);
      window.removeEventListener('resize', place);
      window.removeEventListener('scroll', onScroll, true);
    };
  }, [open, onClose, place, anchor]);

  return { menuRef, pos };
}

const BADGE_CLS: Record<'ok' | 'warn' | 'muted', string> = {
  ok: 'text-[var(--added-fg)] bg-[var(--added-bg)] border-[color-mix(in_srgb,var(--added-fg)_26%,transparent)]',
  warn: 'text-[var(--modified-fg)] bg-[color-mix(in_srgb,var(--modified-fg)_12%,transparent)] border-[color-mix(in_srgb,var(--modified-fg)_28%,transparent)]',
  muted: 'text-[var(--text-dim)] bg-[var(--bg-base)] border-[var(--border)]',
};

const DOT_CLS: Record<'ok' | 'warn' | 'muted', string> = {
  ok: 'bg-[var(--added-fg)]',
  warn: 'bg-[var(--modified-fg)]',
  muted: 'border border-[var(--text-dim)]/70 bg-transparent',
};

const Options: React.FC<{
  options: SelectOption[];
  value: string;
  active: number;
  mono?: boolean;
  onPick: (o: SelectOption) => void;
  onHover: (i: number) => void;
  listRef: React.RefObject<HTMLDivElement | null>;
}> = ({ options, value, active, mono, onPick, onHover, listRef }) => {
  let lastGroup: string | undefined;
  return (
    <div ref={listRef} role="listbox" className="overflow-y-auto py-1 flex-1 min-h-0">
      {options.map((o, i) => {
        const hadGroup = lastGroup !== undefined;
        const header = o.group && o.group !== lastGroup ? o.group : null;
        lastGroup = o.group ?? lastGroup;
        const selected = o.value === value;
        const highlighted = i === active;
        return (
          <React.Fragment key={o.value}>
            {header && (
              <div
                className={`px-3 pb-1 text-[10px] font-semibold uppercase tracking-wider text-[var(--text-dim)] ${
                  hadGroup ? 'mt-1 pt-2 border-t border-[var(--border)]/70' : 'pt-1.5'
                }`}
              >
                {header}
              </div>
            )}
            <button
              type="button"
              role="option"
              aria-selected={selected}
              data-idx={i}
              disabled={o.disabled}
              onMouseDown={(e) => e.preventDefault()}
              onMouseEnter={() => onHover(i)}
              onClick={() => onPick(o)}
              className={`w-full flex items-center gap-2.5 px-3 py-1.5 text-left cursor-pointer disabled:opacity-40 disabled:cursor-default transition-colors ${
                highlighted
                  ? 'bg-[var(--bg-hover)]'
                  : selected
                    ? 'bg-[color-mix(in_srgb,var(--accent)_8%,transparent)]'
                    : ''
              }`}
            >
              {o.dot && <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${DOT_CLS[o.dot]}`} />}
              {o.icon && (
                <span
                  className={`shrink-0 ${
                    selected ? 'text-[var(--accent)]' : 'text-[var(--text-muted)]'
                  }`}
                >
                  {o.icon}
                </span>
              )}
              <span className="min-w-0 flex-1">
                <span
                  className={`block truncate text-[var(--text-primary)] ${
                    mono ? 'font-mono text-[12px]' : 'text-[13px]'
                  } ${selected ? 'font-medium' : ''}`}
                >
                  {o.label}
                </span>
                {o.hint && (
                  <span className="block truncate text-[11px] leading-snug text-[var(--text-dim)]">{o.hint}</span>
                )}
              </span>
              {o.badge && (
                <span
                  className={`shrink-0 rounded-full px-1.5 py-px text-[10.5px] font-medium border ${
                    BADGE_CLS[o.badgeTone ?? 'muted']
                  }`}
                >
                  {o.badge}
                </span>
              )}
              <span className="w-3.5 shrink-0 flex items-center justify-center">
                {selected && <Check size={13} className="text-[var(--accent)]" />}
              </span>
            </button>
          </React.Fragment>
        );
      })}
    </div>
  );
};

const menuCls =
  'fixed z-[100] flex flex-col rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-[0_12px_32px_-8px_rgba(0,0,0,0.48),0_2px_8px_rgba(0,0,0,0.2)] overflow-hidden';

const focusRingCls =
  'border-[var(--accent)] shadow-[0_0_0_3px_color-mix(in_srgb,var(--accent)_18%,transparent)]';

const triggerCls = (open: boolean) =>
  `w-full h-9 flex items-center gap-2 px-2.5 rounded-lg border text-left cursor-pointer transition-[border-color,box-shadow,background-color] outline-none ${
    open
      ? `bg-[var(--bg-input)] ${focusRingCls}`
      : 'bg-[var(--bg-input)] border-[var(--border)] hover:border-[color-mix(in_srgb,var(--text-dim)_65%,var(--border))] focus-visible:border-[var(--accent)] focus-visible:shadow-[0_0_0_3px_color-mix(in_srgb,var(--accent)_18%,transparent)]'
  }`;

function useKeyNav(
  options: SelectOption[],
  open: boolean,
  listRef: React.RefObject<HTMLDivElement | null>,
  initial: number
) {
  const [active, setActive] = useState(initial);
  useEffect(() => {
    if (open) setActive(initial);
  }, [open, initial]);
  useEffect(() => {
    if (active >= options.length) setActive(Math.max(0, options.length - 1));
  }, [active, options.length]);
  useEffect(() => {
    if (!open) return;
    listRef.current?.querySelector<HTMLElement>(`[data-idx="${active}"]`)?.scrollIntoView({ block: 'nearest' });
  }, [active, open, listRef]);
  const move = (dir: 1 | -1) => {
    if (!options.length) return;
    let i = active;
    for (let n = 0; n < options.length; n++) {
      i = (i + dir + options.length) % options.length;
      if (!options[i].disabled) break;
    }
    setActive(i);
  };
  return { active, setActive, move };
}

/* ------------------------------------------------------------------------- */
/* Select                                                                    */
/* ------------------------------------------------------------------------- */

export const Select: React.FC<{
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  placeholder?: string;
  mono?: boolean;
  ariaLabel?: string;
}> = ({ value, options, onChange, placeholder, mono, ariaLabel }) => {
  const [open, setOpen] = useState(false);
  const btnRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const close = useCallback(() => setOpen(false), []);
  const { menuRef, pos } = usePopover(open, close, btnRef);
  const selectedIdx = options.findIndex((o) => o.value === value);
  const selected = options[selectedIdx];
  const { active, setActive, move } = useKeyNav(options, open, listRef, Math.max(0, selectedIdx));

  const pick = (o: SelectOption) => {
    if (o.disabled) return;
    onChange(o.value);
    close();
    btnRef.current?.focus();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!open) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp' || e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        setOpen(true);
      }
      return;
    }
    if (e.key === 'ArrowDown') (e.preventDefault(), move(1));
    else if (e.key === 'ArrowUp') (e.preventDefault(), move(-1));
    else if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      if (options[active]) pick(options[active]);
    } else if (e.key === 'Escape' || e.key === 'Tab') {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
      }
      close();
    }
  };

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        onClick={() => setOpen((v) => !v)}
        onKeyDown={onKeyDown}
        className={triggerCls(open)}
      >
        {selected?.dot && <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${DOT_CLS[selected.dot]}`} />}
        {selected?.icon && <span className="shrink-0 text-[var(--text-muted)]">{selected.icon}</span>}
        <span
          className={`min-w-0 flex-1 truncate ${mono ? 'font-mono text-[12px]' : 'text-[13px] font-medium'} ${
            selected ? 'text-[var(--text-primary)]' : 'text-[var(--text-dim)]'
          }`}
        >
          {selected?.label ?? placeholder ?? 'Select…'}
        </span>
        {selected?.badge && (
          <span
            className={`shrink-0 rounded-full px-1.5 py-px text-[10.5px] font-medium border ${
              BADGE_CLS[selected.badgeTone ?? 'muted']
            }`}
          >
            {selected.badge}
          </span>
        )}
        <ChevronDown
          size={14}
          className={`shrink-0 text-[var(--text-muted)] transition-transform duration-150 ${open ? 'rotate-180' : ''}`}
        />
      </button>
      {open &&
        pos &&
        createPortal(
          <div
            ref={menuRef}
            className={menuCls}
            style={{ left: pos.left, width: pos.width, top: pos.top, bottom: pos.bottom, maxHeight: pos.maxHeight }}
          >
            <Options
              options={options}
              value={value}
              active={active}
              mono={mono}
              onPick={pick}
              onHover={setActive}
              listRef={listRef}
            />
          </div>,
          document.body
        )}
    </>
  );
};

/* ------------------------------------------------------------------------- */
/* Combobox: free text with suggestions                                      */
/* ------------------------------------------------------------------------- */

export const Combobox: React.FC<{
  value: string;
  suggestions: (string | SelectOption)[];
  onChange: (value: string) => void;
  placeholder?: string;
  mono?: boolean;
  ariaLabel?: string;
  trailing?: React.ReactNode;
}> = ({ value, suggestions, onChange, placeholder, mono = true, ariaLabel, trailing }) => {
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState(false);
  const [navigated, setNavigated] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const close = useCallback(() => {
    setOpen(false);
    setNavigated(false);
  }, []);
  const { menuRef, pos } = usePopover(open, close, wrapRef);

  const normalized = useMemo<SelectOption[]>(
    () => suggestions.map((s) => (typeof s === 'string' ? { value: s, label: s } : s)),
    [suggestions]
  );

  // Show everything when just opened; filter once the user starts typing.
  const options = useMemo<SelectOption[]>(() => {
    const q = value.trim().toLowerCase();
    if (!typed || !q) return normalized;
    return normalized.filter(
      (o) =>
        o.value.toLowerCase().includes(q) ||
        o.label.toLowerCase().includes(q) ||
        (o.hint && o.hint.toLowerCase().includes(q))
    );
  }, [normalized, value, typed]);

  const selectedIdx = options.findIndex((o) => o.value === value);
  const { active, setActive, move } = useKeyNav(options, open, listRef, Math.max(0, selectedIdx));

  const pick = (o: SelectOption) => {
    onChange(o.value);
    setTyped(false);
    close();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (!open) {
        if (normalized.length > 0) setOpen(true);
      } else {
        setNavigated(true);
        move(1);
      }
    } else if (e.key === 'ArrowUp' && open) {
      e.preventDefault();
      setNavigated(true);
      move(-1);
    } else if (e.key === 'Enter' && open && options[active] && (navigated || typed)) {
      e.preventDefault();
      pick(options[active]);
    } else if (e.key === 'Escape' || e.key === 'Enter' || e.key === 'Tab') {
      if (open && e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
      }
      close();
    }
  };

  const hasList = normalized.length > 0;

  return (
    <div className="flex w-full items-center gap-2">
      <div
        ref={wrapRef}
        className={`relative flex-1 min-w-0 h-9 flex items-center rounded-lg border bg-[var(--bg-input)] transition-[border-color,box-shadow] ${
          open
            ? focusRingCls
            : 'border-[var(--border)] hover:border-[color-mix(in_srgb,var(--text-dim)_65%,var(--border))] focus-within:border-[var(--accent)] focus-within:shadow-[0_0_0_3px_color-mix(in_srgb,var(--accent)_18%,transparent)]'
        }`}
      >
        <input
          ref={inputRef}
          value={value}
          aria-label={ariaLabel}
          role="combobox"
          aria-expanded={open}
          aria-autocomplete="list"
          onClick={() => {
            if (hasList && !open) {
              setTyped(false);
              setOpen(true);
            }
          }}
          onChange={(e) => {
            onChange(e.target.value);
            setTyped(true);
            setNavigated(false);
            if (hasList) setOpen(true);
          }}
          onFocus={() => setTyped(false)}
          onKeyDown={onKeyDown}
          placeholder={placeholder}
          spellCheck={false}
          autoComplete="off"
          className={`h-full min-w-0 flex-1 bg-transparent pl-2.5 pr-1 outline-none text-[var(--text-primary)] placeholder:text-[var(--text-dim)] text-ellipsis ${
            mono ? 'font-mono text-[12px]' : 'text-[13px]'
          }`}
        />
        {hasList && (
          <button
            type="button"
            tabIndex={-1}
            aria-label="Toggle suggestions"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              setTyped(false);
              setOpen((v) => !v);
              inputRef.current?.focus();
            }}
            className="h-full px-2.5 flex items-center text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer transition-colors"
          >
            <ChevronDown size={14} className={`transition-transform duration-150 ${open ? 'rotate-180' : ''}`} />
          </button>
        )}
      </div>
      {trailing}
      {open &&
        pos &&
        options.length > 0 &&
        createPortal(
          <div
            ref={menuRef}
            className={menuCls}
            style={{ left: pos.left, width: pos.width, top: pos.top, bottom: pos.bottom, maxHeight: pos.maxHeight }}
          >
            <Options
              options={options}
              value={value}
              active={active}
              mono={mono}
              onPick={pick}
              onHover={(i) => {
                setNavigated(true);
                setActive(i);
              }}
              listRef={listRef}
            />
            {normalized.length > 6 && (
              <div className="px-3 py-1.5 border-t border-[var(--border)]/70 bg-[var(--bg-panel)]/60 text-[11px] text-[var(--text-dim)] flex items-center justify-between">
                <span>
                  {options.length === normalized.length
                    ? `${normalized.length} models available`
                    : `${options.length} of ${normalized.length} matching`}
                </span>
                <span>Type any custom ID</span>
              </div>
            )}
          </div>,
          document.body
        )}
    </div>
  );
};
