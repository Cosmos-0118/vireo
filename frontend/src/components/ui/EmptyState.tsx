import type { ReactNode } from 'react';
import type { Icon } from '@phosphor-icons/react';

type Tone = 'neutral' | 'success' | 'accent';

const TONE_FG: Record<Tone, string> = {
  neutral: 'var(--text-muted)',
  success: 'var(--added-fg)',
  accent: 'var(--accent)',
};

/**
 * Panel-level placeholder: a quiet icon tile, a short title, one line of guidance and an
 * optional action. Sized for narrow sidebars by default; `size="lg"` for the editor stage.
 */
export function EmptyState({
  icon: IconCmp,
  title,
  description,
  action,
  tone = 'neutral',
  size = 'md',
  className = '',
  style,
}: {
  icon: Icon;
  title: ReactNode;
  description?: ReactNode;
  action?: ReactNode;
  tone?: Tone;
  size?: 'sm' | 'md' | 'lg';
  className?: string;
  style?: React.CSSProperties;
}) {
  const fg = TONE_FG[tone];
  const tile = size === 'lg' ? 48 : size === 'sm' ? 32 : 40;
  const glyph = size === 'lg' ? 24 : size === 'sm' ? 16 : 20;
  return (
    <div
      role="status"
      style={style}
      className={`vx-empty flex flex-col items-center justify-center text-center min-h-0 overflow-hidden ${
        size === 'sm' ? 'gap-2 px-4 py-5' : size === 'lg' ? 'gap-3.5 p-8' : 'gap-3 px-5 py-8'
      } ${className}`}
    >
      <span
        aria-hidden="true"
        className="vx-empty-tile relative grid place-items-center shrink-0"
        style={{
          width: tile,
          height: tile,
          borderRadius: size === 'lg' ? 12 : size === 'sm' ? 8 : 10,
          color: fg,
          ['--vx-empty-tone' as string]: fg,
        }}
      >
        <IconCmp size={glyph} weight="duotone" />
      </span>
      <div className={`flex flex-col items-center ${size === 'lg' ? 'gap-1.5 max-w-[340px]' : 'gap-1 max-w-[240px]'}`}>
        <div
          className={`font-medium tracking-[-0.01em] text-[var(--text-primary)] ${
            size === 'lg' ? 'text-[14px]' : size === 'sm' ? 'text-[12px]' : 'text-[12.5px]'
          }`}
        >
          {title}
        </div>
        {description && (
          <div
            className={`leading-[1.5] text-[var(--text-dim)] break-words max-w-full ${
              size === 'lg' ? 'text-[12.5px]' : 'text-[11.5px]'
            }`}
          >
            {description}
          </div>
        )}
      </div>
      {action && <div className="flex flex-wrap items-center justify-center gap-1.5 mt-0.5">{action}</div>}
    </div>
  );
}

/** Secondary button sized for empty states. */
export function EmptyStateButton({
  children,
  onClick,
  icon: IconCmp,
  primary = false,
}: {
  children: ReactNode;
  onClick: () => void;
  icon?: Icon;
  primary?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      className={`h-[26px] px-2.5 inline-flex items-center gap-1.5 rounded-md text-[12px] font-medium cursor-pointer transition-colors ${
        primary
          ? 'bg-[var(--accent)] text-[var(--accent-fg)] hover:brightness-110'
          : 'border border-[var(--border)] bg-[var(--bg-elevated)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
      }`}
    >
      {IconCmp && <IconCmp size={13} weight="bold" />}
      {children}
    </button>
  );
}

/** Inline "nothing here" row for menus, pickers and short lists. */
export function EmptyRow({
  icon: IconCmp,
  children,
  className = '',
}: {
  icon?: Icon;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      role="status"
      className={`flex items-center gap-2 px-3 py-2 text-[12px] text-[var(--text-dim)] ${className}`}
    >
      {IconCmp && <IconCmp size={14} weight="duotone" className="shrink-0 opacity-80" />}
      <span className="min-w-0">{children}</span>
    </div>
  );
}
