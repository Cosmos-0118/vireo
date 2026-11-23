import React, { useEffect, useRef } from 'react';
import type { SlashCommand } from '../../types';

interface SlashCommandMenuProps {
  items: SlashCommand[];
  selected: number;
  /** The command list is still being fetched from the agent. */
  loading: boolean;
  onPick: (command: SlashCommand) => void;
}

const SOURCE_LABEL: Record<string, string> = { skill: 'skill', project: 'project', user: 'user' };

/** Popover above the composer listing the agent's slash commands and skills. */
export const SlashCommandMenu: React.FC<SlashCommandMenuProps> = ({ items, selected, loading, onPick }) => {
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>('[data-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [selected]);

  return (
    <div className="absolute left-2 right-2 bottom-full mb-1 z-40 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl overflow-hidden">
      <div ref={listRef} className="max-h-64 overflow-y-auto py-1">
        {items.length === 0 ? (
          <div className="px-3 py-2 text-[12px] text-[var(--text-dim)]">
            {loading ? 'Loading commands…' : 'No matching commands'}
          </div>
        ) : (
          items.map((c, i) => (
            <div
              key={c.name}
              data-selected={i === selected}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => onPick(c)}
              className={`flex items-baseline gap-2 px-3 py-1 cursor-pointer hover:bg-[var(--bg-hover)] ${
                i === selected ? 'bg-[var(--bg-active)]' : ''
              }`}
              title={c.description}
            >
              <span className="shrink-0 text-[12px] font-mono text-[var(--text-primary)]">/{c.name}</span>
              {c.hint && <span className="shrink-0 text-[11px] font-mono text-[var(--text-dim)]">{c.hint}</span>}
              <span className="min-w-0 flex-1 truncate text-[11.5px] text-[var(--text-muted)]">{c.description}</span>
              {SOURCE_LABEL[c.source] && (
                <span className="shrink-0 px-1 rounded bg-[var(--bg-hover)] text-[10px] text-[var(--text-dim)]">
                  {SOURCE_LABEL[c.source]}
                </span>
              )}
            </div>
          ))
        )}
      </div>
    </div>
  );
};
