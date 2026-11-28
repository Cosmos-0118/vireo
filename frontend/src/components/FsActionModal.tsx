import React, { useEffect, useRef, useState } from 'react';
import { AlertTriangle, FilePlus, FolderPlus, Pencil, X } from 'lucide-react';

export type FsModalState =
  | { kind: 'new-file'; dir: string }
  | { kind: 'new-folder'; dir: string }
  | { kind: 'rename'; path: string; isDir: boolean }
  | { kind: 'delete'; path: string; isDir: boolean }
  | null;

interface FsActionModalProps {
  modal: FsModalState;
  busy?: boolean;
  onClose: () => void;
  /** `value` is the typed name (new/rename) — the caller resolves it against the modal's path. */
  onConfirm: (modal: NonNullable<FsModalState>, value: string) => void;
}

const btnCancel =
  'px-3 py-1.5 text-xs rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer';
const btnPrimary =
  'px-3.5 py-1.5 text-xs font-medium rounded-md bg-[var(--accent)] text-white hover:opacity-90 disabled:opacity-40 cursor-pointer';
const btnDanger =
  'px-3.5 py-1.5 text-xs font-medium rounded-md bg-[var(--danger)] text-white hover:opacity-90 disabled:opacity-40 cursor-pointer';
const closeBtn =
  'p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer';

function initialValue(modal: FsModalState): string {
  if (modal?.kind === 'rename') return modal.path.slice(modal.path.lastIndexOf('/') + 1);
  return '';
}

export const FsActionModal: React.FC<FsActionModalProps> = ({ modal, busy = false, onClose, onConfirm }) => {
  const [value, setValue] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setValue(initialValue(modal));
    if (modal && modal.kind !== 'delete') {
      // Select the stem of a file name so typing replaces it but keeps the extension.
      requestAnimationFrame(() => {
        const el = inputRef.current;
        if (!el) return;
        el.focus();
        const dot = el.value.lastIndexOf('.');
        if (modal.kind === 'rename' && !modal.isDir && dot > 0) el.setSelectionRange(0, dot);
        else el.select();
      });
    }
  }, [modal]);

  useEffect(() => {
    if (!modal) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [modal, onClose]);

  if (!modal) return null;

  const trimmed = value.trim();
  const invalid = /[<>:"|?*\u0000]/.test(trimmed) || trimmed === '.' || trimmed === '..';
  const unchanged = modal.kind === 'rename' && trimmed === initialValue(modal);
  const canSubmit = !busy && !!trimmed && !invalid && !unchanged;

  const header = (icon: React.ReactNode, title: string, danger = false) => (
    <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border)]">
      <div
        className={`flex items-center gap-2 font-semibold text-sm ${danger ? 'text-[var(--removed-fg)]' : ''}`}
      >
        {icon}
        {title}
      </div>
      <button type="button" onClick={onClose} className={closeBtn}>
        <X size={15} />
      </button>
    </div>
  );

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/55 backdrop-blur-[1px] p-4"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="w-full max-w-[440px] rounded-xl border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl overflow-hidden text-[var(--text-primary)]">
        {modal.kind === 'delete' ? (
          <div>
            {header(<AlertTriangle size={16} />, modal.isDir ? 'Delete Folder' : 'Delete File', true)}
            <div className="p-4 space-y-2.5 text-xs">
              <p className="text-[var(--text-muted)]">
                {modal.isDir
                  ? 'Permanently delete this folder and everything inside it? This cannot be undone.'
                  : 'Permanently delete this file? This cannot be undone.'}
              </p>
              <div className="px-3 py-2 rounded-md bg-[var(--bg-input)] border border-[var(--border)] font-mono text-[var(--text-primary)] break-all">
                {modal.path}
              </div>
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)]/50">
              <button type="button" onClick={onClose} className={btnCancel}>
                Cancel
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => onConfirm(modal, '')}
                className={btnDanger}
              >
                Delete
              </button>
            </div>
          </div>
        ) : (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (canSubmit) onConfirm(modal, trimmed);
            }}
          >
            {modal.kind === 'rename'
              ? header(<Pencil size={16} className="text-[var(--accent)]" />, modal.isDir ? 'Rename Folder' : 'Rename File')
              : modal.kind === 'new-file'
              ? header(<FilePlus size={16} className="text-[var(--accent)]" />, 'New File')
              : header(<FolderPlus size={16} className="text-[var(--accent)]" />, 'New Folder')}
            <div className="p-4 space-y-2.5 text-xs">
              {modal.kind !== 'rename' && (
                <p className="text-[var(--text-muted)]">
                  Create in{' '}
                  <code className="font-mono text-[var(--text-primary)]">{modal.dir || 'workspace root'}</code>
                  . Use <code className="font-mono">/</code> to create nested folders.
                </p>
              )}
              <input
                ref={inputRef}
                value={value}
                onChange={(e) => setValue(e.target.value)}
                spellCheck={false}
                autoComplete="off"
                placeholder={modal.kind === 'new-folder' ? 'folder-name' : 'file-name.ext'}
                className="w-full px-2.5 py-1.5 rounded-md bg-[var(--bg-input)] border border-[var(--border)] focus:border-[var(--accent)] outline-none font-mono text-[var(--text-primary)]"
              />
              {invalid && (
                <p className="text-[var(--removed-fg)]">Names can't contain {'< > : " | ? *'}.</p>
              )}
            </div>
            <div className="flex justify-end gap-2 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)]/50">
              <button type="button" onClick={onClose} className={btnCancel}>
                Cancel
              </button>
              <button type="submit" disabled={!canSubmit} className={btnPrimary}>
                {modal.kind === 'rename' ? 'Rename' : 'Create'}
              </button>
            </div>
          </form>
        )}
      </div>
    </div>
  );
};
