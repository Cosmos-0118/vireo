import React, { useEffect, useRef } from 'react';

// Chunky pixel "V" (# = filled).
const V_SHAPE = [
  '##.......##',
  '.##.....##.',
  '.##.....##.',
  '..##...##..',
  '..##...##..',
  '...##.##...',
  '....###....',
  '.....#.....',
];

const COLS = 15;
const ROWS = 14;
const OFFSET_X = 2;
const OFFSET_Y = 5;
const CELL = 12;
const GAP = 2;
const FPS = 10;

const SPARKLES = [
  { x: 3, y: 9, speed: 1.1 },
  { x: 5, y: 4, speed: 0.8 },
  { x: 7, y: 12, speed: 1.4 },
  { x: 9, y: 7, speed: 0.9 },
  { x: 11, y: 2, speed: 1.2 },
  { x: 13, y: 10, speed: 0.7 },
  { x: 1, y: 6, speed: 1.0 },
];

function PixelV() {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current;
    const ctx = canvas?.getContext('2d');
    if (!canvas || !ctx) return;

    const width = COLS * CELL;
    const height = ROWS * CELL;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = width * dpr;
    canvas.height = height * dpr;
    ctx.scale(dpr, dpr);

    const color =
      getComputedStyle(canvas).getPropertyValue('--accent').trim() || '#3b82f6';
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

    const px = (x: number, y: number, alpha: number) => {
      ctx.globalAlpha = alpha;
      ctx.fillRect(x * CELL, y * CELL, CELL - GAP, CELL - GAP);
    };

    const draw = (t: number) => {
      ctx.clearRect(0, 0, width, height);
      ctx.fillStyle = color;

      V_SHAPE.forEach((row, ry) => {
        for (let rx = 0; rx < row.length; rx++) {
          if (row[rx] !== '#') continue;
          // Diagonal shimmer sweeping across the V.
          const wave = Math.sin((rx + ry) * 0.7 - t * 2.4);
          const alpha = reduced ? 0.9 : 0.55 + 0.45 * ((wave + 1) / 2);
          px(rx + OFFSET_X, ry + OFFSET_Y, alpha);
        }
      });

      if (!reduced) {
        for (const s of SPARKLES) {
          const travel = (s.y - t * s.speed) % OFFSET_Y;
          const y = Math.floor(travel < 0 ? travel + OFFSET_Y : travel);
          const fade = 1 - y / OFFSET_Y;
          px(s.x, y, 0.15 + 0.5 * fade);
        }
      }
      ctx.globalAlpha = 1;
    };

    if (reduced) {
      draw(0);
      return;
    }

    let raf = 0;
    let last = 0;
    const start = performance.now();
    const loop = (now: number) => {
      raf = requestAnimationFrame(loop);
      if (now - last < 1000 / FPS) return;
      last = now;
      draw((now - start) / 1000);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, []);

  return (
    <canvas
      ref={ref}
      style={{ width: COLS * CELL, height: ROWS * CELL }}
      aria-hidden="true"
      className="select-none"
    />
  );
}

interface WelcomeAction {
  label: string;
  onClick: () => void;
  primary?: boolean;
}

export const WelcomeScreen: React.FC<{ actions: WelcomeAction[] }> = ({ actions }) => (
  <div className="flex-1 min-h-0 overflow-y-auto flex flex-col items-center justify-center-safe p-8 gap-6">
    <PixelV />
    <h2 className="text-xl font-semibold tracking-tight text-[var(--text-primary)]">Vireo</h2>
    <div className="flex flex-wrap items-center justify-center gap-2 text-[13px]">
      {actions.map((a) => (
        <button
          key={a.label}
          onClick={a.onClick}
          className={
            a.primary
              ? 'px-3.5 py-1.5 rounded-md bg-[var(--accent)] text-[var(--accent-fg)] font-medium cursor-pointer hover:opacity-90 transition-opacity'
              : 'px-3.5 py-1.5 rounded-md text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer transition-colors'
          }
        >
          {a.label}
        </button>
      ))}
    </div>
  </div>
);
