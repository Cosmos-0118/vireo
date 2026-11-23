import { useCallback, useMemo, useState } from 'react';
import type { SlashCommand } from '../../types';

interface SlashRange {
  /** Index of the `/` in the composer text. */
  start: number;
  end: number;
  query: string;
}

const MAX_ITEMS = 60;

/** Name matches ranked: prefix, then word start, then anywhere; description-only matches last. */
export function filterCommands(commands: SlashCommand[], query: string): SlashCommand[] {
  const q = query.toLowerCase();
  if (!q) return commands.slice(0, MAX_ITEMS);
  const rank = (c: SlashCommand): number => {
    const name = c.name.toLowerCase();
    if (name.startsWith(q)) return 0;
    if (name.split(/[:\-_]/).some((w) => w.startsWith(q))) return 1;
    if (name.includes(q)) return 2;
    return c.description.toLowerCase().includes(q) ? 3 : -1;
  };
  return commands
    .map((c) => ({ c, r: rank(c) }))
    .filter((x) => x.r >= 0)
    .sort((a, b) => a.r - b.r || a.c.name.localeCompare(b.c.name))
    .slice(0, MAX_ITEMS)
    .map((x) => x.c);
}

/** A slash command is only meaningful as the first word of a message. */
export function detectSlash(text: string, caret: number): SlashRange | null {
  const m = text.slice(0, caret).match(/^(\s*)\/([^\s/]*)$/);
  return m ? { start: m[1].length, end: caret, query: m[2] } : null;
}

/**
 * State of the composer's `/` popover. `onOpen` runs when the popover opens so the caller can
 * fetch the agent's commands lazily.
 */
export function useSlashMenu(commands: SlashCommand[], onOpen: () => void) {
  const [range, setRange] = useState<SlashRange | null>(null);
  const [index, setIndex] = useState(0);
  const items = useMemo(() => (range ? filterCommands(commands, range.query) : []), [commands, range]);

  const update = useCallback(
    (text: string, caret: number) => {
      const next = detectSlash(text, caret);
      if (next && !range) onOpen();
      setRange(next);
      setIndex(0);
    },
    [onOpen, range]
  );

  const close = useCallback(() => setRange(null), []);

  /** Composer text after choosing `command`: `/name ` followed by whatever came after the caret. */
  const complete = useCallback(
    (text: string, command: SlashCommand): string => {
      if (!range) return text;
      return `${text.slice(0, range.start)}/${command.name} ${text.slice(range.end).replace(/^\s+/, '')}`;
    },
    [range]
  );

  return {
    open: range !== null,
    items,
    index: Math.min(index, Math.max(items.length - 1, 0)),
    move: (delta: number) => setIndex((i) => Math.max(0, Math.min(i + delta, items.length - 1))),
    update,
    close,
    complete,
  };
}
