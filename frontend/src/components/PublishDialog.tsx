import React, { useEffect, useState } from 'react';
import { AlertTriangle, Globe, Link2, Loader2, Lock, X } from 'lucide-react';
import { api } from '../api';
import type { GithubStatusRes } from '../types';

export type PublishMode = 'github' | 'remote';

interface PublishDialogProps {
  open: boolean;
  /** Initial tab. */
  initialMode?: PublishMode;
  /** Suggested repository name (usually the folder name). */
  defaultName: string;
  /** False when the folder isn't a git repo yet: publishing will initialize it. */
  isRepo: boolean;
  /** True when HEAD has no commits: an initial commit will be made. */
  hasCommits: boolean;
  busy: boolean;
  onClose: () => void;
  onPublishGithub: (name: string, isPrivate: boolean, description: string) => void;
  onAddRemote: (url: string) => void;
}

const NAME_RE = /^[A-Za-z0-9._-]+$/;

function sanitizeName(raw: string): string {
  return raw
    .trim()
    .replace(/\s+/g, '-')
    .replace(/[^A-Za-z0-9._-]/g, '')
    .replace(/^-+/, '');
}

export const PublishDialog: React.FC<PublishDialogProps> = ({
  open,
  initialMode = 'github',
  defaultName,
  isRepo,
  hasCommits,
  busy,
  onClose,
  onPublishGithub,
  onAddRemote,
}) => {
  const [mode, setMode] = useState<PublishMode>(initialMode);
  const [name, setName] = useState('');
  const [isPrivate, setIsPrivate] = useState(true);
  const [description, setDescription] = useState('');
  const [remoteUrl, setRemoteUrl] = useState('');
  const [gh, setGh] = useState<GithubStatusRes | null>(null);
  const [ghLoading, setGhLoading] = useState(false);

  useEffect(() => {
    if (!open) return;
    setMode(initialMode);
    setName(sanitizeName(defaultName));
    setIsPrivate(true);
    setDescription('');
    setRemoteUrl('');
    setGh(null);
    setGhLoading(true);
    let cancelled = false;
    api
      .githubStatus()
      .then((s) => !cancelled && setGh(s))
      .catch(() => !cancelled && setGh({ installed: false, authenticated: false, login: '' }))
      .finally(() => !cancelled && setGhLoading(false));
    return () => {
      cancelled = true;
    };
  }, [open, initialMode, defaultName]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onClose]);

  if (!open) return null;

  const ghReady = !!gh?.installed && !!gh?.authenticated;
  const nameValid = NAME_RE.test(name) && name.length <= 100;
  const urlValid = /^(https?:\/\/|ssh:\/\/|git:\/\/|git@)\S+$/.test(remoteUrl.trim());

  const ghHint = ghLoading
    ? 'Checking GitHub CLI…'
    : !gh?.installed
      ? 'GitHub CLI (gh) is not installed. Install it from cli.github.com, or use "Existing remote URL".'
      : !gh.authenticated
        ? 'Not signed in to GitHub. Run `gh auth login` in the terminal, then reopen this dialog.'
        : '';

  const initNote = !isRepo
    ? 'This folder will be initialized as a Git repository.'
    : !hasCommits
      ? 'Your files will be committed as “Initial commit” first.'
      : '';

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (busy) return;
    if (mode === 'github') {
      if (ghReady && nameValid) onPublishGithub(name, isPrivate, description.trim());
    } else if (urlValid) {
      onAddRemote(remoteUrl.trim());
    }
  };

  const tab = (m: PublishMode, label: string, icon: React.ReactNode) => (
    <button
      type="button"
      onClick={() => setMode(m)}
      className={`flex-1 flex items-center justify-center gap-1.5 py-1.5 text-[12px] font-medium rounded-md cursor-pointer ${
        mode === m
          ? 'bg-[var(--bg-hover)] text-[var(--text-primary)]'
          : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
      }`}
    >
      {icon}
      {label}
    </button>
  );

  const visibilityOption = (priv: boolean, icon: React.ReactNode, title: string, hint: string) => (
    <button
      type="button"
      onClick={() => setIsPrivate(priv)}
      className={`flex-1 flex items-start gap-2 px-2.5 py-2 rounded-md border text-left cursor-pointer ${
        isPrivate === priv
          ? 'border-[var(--accent)] bg-[var(--accent)]/10'
          : 'border-[var(--border)] hover:bg-[var(--bg-hover)]'
      }`}
    >
      <span className="mt-[1px] text-[var(--text-muted)]">{icon}</span>
      <span className="min-w-0">
        <span className="block text-[12px] font-medium text-[var(--text-primary)]">{title}</span>
        <span className="block text-[10.5px] leading-snug text-[var(--text-dim)]">{hint}</span>
      </span>
    </button>
  );

  const inputCls =
    'w-full px-2.5 py-1.5 text-[12.5px] rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none focus:border-[var(--accent)]';

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/55 backdrop-blur-[1px] p-4"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <form
        onSubmit={submit}
        className="w-full max-w-[440px] rounded-xl border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl overflow-hidden text-[var(--text-primary)]"
      >
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border)]">
          <span className="font-semibold text-sm">Publish to remote</span>
          <button
            type="button"
            onClick={onClose}
            className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer"
            aria-label="Close"
          >
            <X size={15} />
          </button>
        </div>

        <div className="p-4 space-y-3 text-xs">
          <div className="flex gap-1 p-0.5 rounded-lg bg-[var(--bg-input)] border border-[var(--border)]">
            {tab('github', 'GitHub', <Globe size={12.5} />)}
            {tab('remote', 'Existing remote URL', <Link2 size={12.5} />)}
          </div>

          {mode === 'github' ? (
            <>
              <label className="block space-y-1">
                <span className="text-[11px] text-[var(--text-muted)]">
                  Repository name
                  {gh?.login ? <span className="text-[var(--text-dim)]"> — {gh.login}/</span> : null}
                </span>
                <input
                  value={name}
                  onChange={(e) => setName(e.target.value.replace(/\s+/g, '-'))}
                  autoFocus
                  spellCheck={false}
                  className={`${inputCls} ${name && !nameValid ? 'border-[var(--removed-fg)]' : ''}`}
                />
                {name && !nameValid && (
                  <span className="block text-[11px] text-[var(--removed-fg)]">
                    Use letters, numbers, “.”, “-” and “_” only.
                  </span>
                )}
              </label>

              <div className="flex gap-2">
                {visibilityOption(true, <Lock size={13} />, 'Private', 'Only you and people you choose')}
                {visibilityOption(false, <Globe size={13} />, 'Public', 'Anyone on the internet')}
              </div>

              <label className="block space-y-1">
                <span className="text-[11px] text-[var(--text-muted)]">Description (optional)</span>
                <input
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  className={inputCls}
                />
              </label>

              {ghHint && (
                <div
                  className={`flex items-start gap-1.5 text-[11px] leading-snug ${
                    ghLoading ? 'text-[var(--text-dim)]' : 'text-[var(--modified-fg)]'
                  }`}
                >
                  {ghLoading ? (
                    <Loader2 size={12} className="mt-[1px] shrink-0 animate-spin" />
                  ) : (
                    <AlertTriangle size={12} className="mt-[1px] shrink-0" />
                  )}
                  <span>{ghHint}</span>
                </div>
              )}
              {initNote && <div className="text-[11px] text-[var(--text-dim)]">{initNote}</div>}
            </>
          ) : (
            <>
              <label className="block space-y-1">
                <span className="text-[11px] text-[var(--text-muted)]">Remote URL</span>
                <input
                  value={remoteUrl}
                  onChange={(e) => setRemoteUrl(e.target.value)}
                  placeholder="https://github.com/owner/repo.git"
                  autoFocus
                  spellCheck={false}
                  className={inputCls}
                />
              </label>
              <div className="text-[11px] text-[var(--text-dim)] leading-snug">
                Adds it as <span className="font-mono">origin</span>
                {isRepo ? '' : ' (the folder will be initialized as a Git repository first)'}. Use “Publish Branch”
                afterwards to push.
              </div>
            </>
          )}
        </div>

        <div className="flex justify-end gap-2 px-4 py-3 border-t border-[var(--border)]">
          <button
            type="button"
            onClick={onClose}
            className="px-3 py-1.5 text-[12px] rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={busy || (mode === 'github' ? !ghReady || !nameValid : !urlValid)}
            className="px-3 py-1.5 text-[12px] font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:brightness-110 disabled:opacity-40 disabled:hover:brightness-100 cursor-pointer disabled:cursor-not-allowed flex items-center gap-1.5"
          >
            {busy && <Loader2 size={12} className="animate-spin" />}
            {mode === 'github' ? `Publish ${isPrivate ? 'Private' : 'Public'} Repository` : 'Add Remote'}
          </button>
        </div>
      </form>
    </div>
  );
};
