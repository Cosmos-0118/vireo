import { useCallback, useRef, useState } from 'react';
import { AlertCircle, Info, X } from 'lucide-react';
import type { ToastAction } from '../types';

export interface ToastData {
  id: number;
  msg: string;
  isError?: boolean;
  action?: ToastAction;
}

const EXIT_MS = 170;

/** Toast duration in ms: errors and actionable toasts linger longer. */
export function toastDuration(t: Pick<ToastData, 'isError' | 'action'>): number {
  if (t.action) return 10000;
  return t.isError ? 7000 : 4500;
}

interface ToastProps {
  toast: ToastData;
  onDismiss: () => void;
}

/**
 * Bottom-right notification card. The countdown bar is a CSS animation whose
 * `animationend` dismisses the toast, so hovering (which pauses the animation)
 * also pauses auto-dismiss without any timer bookkeeping.
 */
export default function Toast({ toast, onDismiss }: ToastProps) {
  const [leaving, setLeaving] = useState(false);
  const closing = useRef(false);

  const close = useCallback(() => {
    if (closing.current) return;
    closing.current = true;
    setLeaving(true);
    window.setTimeout(onDismiss, EXIT_MS);
  }, [onDismiss]);

  const tone = toast.isError ? 'var(--danger)' : 'var(--accent)';
  const Icon = toast.isError ? AlertCircle : Info;

  return (
    <div
      className="vx-toast-wrap"
      role={toast.isError ? 'alert' : 'status'}
      aria-live={toast.isError ? 'assertive' : 'polite'}
    >
      <div
        className={`vx-toast${leaving ? ' vx-toast-leave' : ''}`}
        style={{ ['--toast-tone' as string]: tone }}
      >
        <div className="vx-toast-body">
          <span className="vx-toast-icon">
            <Icon size={16} strokeWidth={2.25} />
          </span>
          <div className="vx-toast-text">
            {toast.isError && <div className="vx-toast-title">Something went wrong</div>}
            <div className={toast.isError ? 'vx-toast-msg vx-toast-msg-sub' : 'vx-toast-msg'}>{toast.msg}</div>
            {toast.action && (
              <button
                type="button"
                className="vx-toast-action"
                onClick={() => {
                  const run = toast.action!.run;
                  closing.current = true;
                  onDismiss();
                  run();
                }}
              >
                {toast.action.label}
              </button>
            )}
          </div>
          <button type="button" className="vx-toast-close" aria-label="Dismiss notification" onClick={close}>
            <X size={14} strokeWidth={2.25} />
          </button>
        </div>
        <div className="vx-toast-track" aria-hidden="true">
          <div
            className="vx-toast-progress"
            style={{ animationDuration: `${toastDuration(toast)}ms` }}
            onAnimationEnd={close}
          />
        </div>
      </div>
    </div>
  );
}
