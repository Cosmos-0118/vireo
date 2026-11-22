import React, { useCallback, useRef, useState } from 'react';

interface ResizeHandleProps {
  /** 'vertical' = a vertical bar that resizes width; 'horizontal' = a horizontal bar that resizes height. */
  orientation: 'vertical' | 'horizontal';
  /** Current (effective) size of the controlled panel in px. */
  value: number;
  min: number;
  max: number;
  /**
   * +1 when moving the pointer right/down grows the panel (panel sits before the handle),
   * -1 when it shrinks it (panel sits after the handle).
   */
  direction: 1 | -1;
  /** Receives an already clamped size. */
  onChange: (next: number) => void;
  onReset?: () => void;
  label: string;
}

const KEY_STEP = 16;
const KEY_STEP_LARGE = 64;

/**
 * Accessible splitter (WAI-ARIA "window splitter"): pointer capture so drags survive leaving the
 * window / crossing xterm & CodeMirror, absolute (start + delta) sizing so the handle never drifts
 * from the cursor when clamped, keyboard resizing, double-click to reset.
 */
const ResizeHandle: React.FC<ResizeHandleProps> = ({
  orientation,
  value,
  min,
  max,
  direction,
  onChange,
  onReset,
  label,
}) => {
  const vertical = orientation === 'vertical';
  const [dragging, setDragging] = useState(false);
  const start = useRef({ pos: 0, size: 0 });

  const clamp = useCallback(
    (n: number) => Math.round(Math.max(min, Math.min(Math.max(min, max), n))),
    [min, max]
  );

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    start.current = { pos: vertical ? e.clientX : e.clientY, size: value };
    setDragging(true);
    document.body.style.cursor = vertical ? 'col-resize' : 'row-resize';
    document.body.style.userSelect = 'none';
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!dragging) return;
    const pos = vertical ? e.clientX : e.clientY;
    const next = clamp(start.current.size + (pos - start.current.pos) * direction);
    if (next !== value) onChange(next);
  };

  const endDrag = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
    setDragging(false);
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const step = e.shiftKey ? KEY_STEP_LARGE : KEY_STEP;
    const grow = vertical ? 'ArrowRight' : 'ArrowDown';
    const shrink = vertical ? 'ArrowLeft' : 'ArrowUp';
    let next: number | null = null;
    if (e.key === grow) next = value + step * direction;
    else if (e.key === shrink) next = value - step * direction;
    else if (e.key === 'Home') next = direction === 1 ? min : max;
    else if (e.key === 'End') next = direction === 1 ? max : min;
    else if (e.key === 'Enter' && onReset) {
      e.preventDefault();
      onReset();
      return;
    }
    if (next === null) return;
    e.preventDefault();
    onChange(clamp(next));
  };

  return (
    <div
      role="separator"
      aria-orientation={vertical ? 'vertical' : 'horizontal'}
      aria-label={label}
      aria-valuenow={Math.round(value)}
      aria-valuemin={min}
      aria-valuemax={Math.max(min, max)}
      tabIndex={0}
      onKeyDown={onKeyDown}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onLostPointerCapture={() => {
        if (dragging) {
          setDragging(false);
          document.body.style.cursor = '';
          document.body.style.userSelect = '';
        }
      }}
      onDoubleClick={onReset}
      style={{ touchAction: 'none' }}
      className={`relative shrink-0 z-10 outline-none transition-colors ${
        vertical ? 'w-1 cursor-col-resize' : 'h-1 cursor-row-resize'
      } ${
        dragging
          ? 'bg-[var(--accent)]'
          : 'hover:bg-[var(--accent)]/60 focus-visible:bg-[var(--accent)]/80'
      } before:content-[''] before:absolute ${
        vertical ? 'before:inset-y-0 before:-inset-x-[3px]' : 'before:inset-x-0 before:-inset-y-[3px]'
      }`}
    />
  );
};

export default ResizeHandle;
