import React, { useEffect, useRef, useState } from 'react';
import { Eye, EyeOff, KeyRound, X } from 'lucide-react';
import type { AskpassPrompt } from '../api';

interface AskpassDialogProps {
  /** The prompt to show, or null when none is waiting. */
  prompt: AskpassPrompt | null;
  /** `null` = cancelled. */
  onSubmit: (id: string, value: string | null) => void;
}

/** Splits git/ssh prompt text into a short title and the detail (key path, URL). */
function describe(text: string): { title: string; detail: string } {
  const t = text.trim().replace(/:\s*$/, '');
  const key = t.match(/^Enter passphrase for (?:key )?["']?(.+?)["']?$/i);
  if (key) return { title: 'SSH key passphrase', detail: key[1] };
  const cred = t.match(/^(Username|Password) for ['"]?(.+?)['"]?$/i);
  if (cred) return { title: cred[1], detail: cred[2] };
  return { title: 'Git needs your input', detail: t };
}

export const AskpassDialog: React.FC<AskpassDialogProps> = ({ prompt, onSubmit }) => {
  const [value, setValue] = useState('');
  const [reveal, setReveal] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const id = prompt?.id;

  useEffect(() => {
    setValue('');
    setReveal(false);
    if (id) window.setTimeout(() => inputRef.current?.focus(), 0);
  }, [id]);

  useEffect(() => {
    if (!id) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onSubmit(id, null);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [id, onSubmit]);

  if (!prompt) return null;
  const { title, detail } = describe(prompt.prompt);
  const hidden = prompt.secret && !reveal;

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    onSubmit(prompt.id, value);
    setValue('');
  };

  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 backdrop-blur-[2px] p-4">
      <form
        role="dialog"
        aria-modal="true"
        aria-labelledby="askpass-title"
        onSubmit={submit}
        className="w-full max-w-[420px] rounded-xl border border-[var(--border)] bg-[var(--bg-elevated)] shadow-[0_24px_64px_-12px_rgba(0,0,0,0.75)] overflow-hidden text-[var(--text-primary)]"
      >
        <div className="flex items-start gap-3 px-4 pt-4">
          <span className="shrink-0 w-8 h-8 rounded-lg flex items-center justify-center bg-[var(--accent)]/15 text-[var(--accent-hover)]">
            <KeyRound size={16} />
          </span>
          <div className="min-w-0 flex-1">
            <h2 id="askpass-title" className="text-[13px] font-semibold">
              {title}
            </h2>
            <p className="mt-0.5 text-[11.5px] text-[var(--text-muted)] break-all" title={prompt.prompt}>
              {detail}
            </p>
          </div>
          <button
            type="button"
            onClick={() => onSubmit(prompt.id, null)}
            className="shrink-0 w-6 h-6 rounded-md flex items-center justify-center text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]"
            aria-label="Cancel"
            title="Cancel"
          >
            <X size={14} />
          </button>
        </div>

        <div className="px-4 pt-3 pb-4">
          <div className="relative">
            <input
              ref={inputRef}
              type={hidden ? 'password' : 'text'}
              value={value}
              onChange={(e) => setValue(e.target.value)}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
              aria-label={title}
              className="w-full h-[32px] pl-2.5 pr-8 text-[12.5px] rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] outline-none transition-colors focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_color-mix(in_srgb,var(--accent)_20%,transparent)]"
            />
            {prompt.secret && (
              <button
                type="button"
                onClick={() => setReveal((r) => !r)}
                className="absolute right-1 top-1 w-6 h-6 rounded flex items-center justify-center text-[var(--text-muted)] hover:text-[var(--text-primary)]"
                aria-label={reveal ? 'Hide' : 'Show'}
                title={reveal ? 'Hide' : 'Show'}
              >
                {reveal ? <EyeOff size={13} /> : <Eye size={13} />}
              </button>
            )}
          </div>
          {prompt.secret && (
            <p className="mt-2 text-[11px] text-[var(--text-dim)]">
              Sent only to git for this operation. Vireo does not store it.
            </p>
          )}
          <div className="mt-4 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => onSubmit(prompt.id, null)}
              className="h-[28px] px-3 text-[12px] rounded-md border border-[var(--border)] text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]"
            >
              Cancel
            </button>
            <button
              type="submit"
              className="h-[28px] px-3 text-[12px] font-medium rounded-md bg-[var(--accent)] text-white hover:bg-[var(--accent-hover)]"
            >
              OK
            </button>
          </div>
        </div>
      </form>
    </div>
  );
};
