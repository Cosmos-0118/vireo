import React, { useEffect, useState } from 'react';
import { AlertTriangle, CloudUpload, Globe, Info, Link2, Loader2, Lock, X } from 'lucide-react';
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
  const canSubmit = !busy && (mode === 'github' ? ghReady && nameValid : urlValid);

  const initNote = !isRepo
    ? 'This folder will be initialized as a Git repository.'
    : !hasCommits
      ? 'Your files will be committed as “Initial commit” first.'
      : '';

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!canSubmit) return;
    if (mode === 'github') onPublishGithub(name, isPrivate, description.trim());
    else onAddRemote(remoteUrl.trim());
  };

  const fieldCls =
    'w-full h-[32px] px-2.5 text-[12.5px] rounded-md bg-[var(--bg-input)] border border-[var(--border)] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none transition-colors focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_color-mix(in_srgb,var(--accent)_20%,transparent)]';
  const labelCls = 'block mb-1.5 text-[11px] font-medium text-[var(--text-muted)]';

  const tab = (m: PublishMode, label: string, icon: React.ReactNode) => (
    <button
      type="button"
      role="tab"
      aria-selected={mode === m}
      onClick={() => setMode(m)}
      className={`flex-1 h-[28px] flex items-center justify-center gap-1.5 text-[12px] font-medium rounded-[6px] cursor-pointer transition-colors ${
        mode === m
          ? 'bg-[var(--bg-active)] text-[var(--text-primary)] shadow-[0_1px_2px_rgba(0,0,0,0.35)]'
          : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
      }`}
    >
      {icon}
      {label}
    </button>
  );

  const visibilityCard = (priv: boolean, icon: React.ReactNode, title: string, hint: string) => {
    const selected = isPrivate === priv;
    return (
      <button
        type="button"
        role="radio"
        aria-checked={selected}
        onClick={() => setIsPrivate(priv)}
        className={`flex-1 min-w-0 flex items-center gap-2.5 px-2.5 py-2 rounded-lg border text-left cursor-pointer transition-colors ${
          selected
            ? 'border-[var(--accent)] bg-[var(--accent)]/10'
            : 'border-[var(--border)] bg-[var(--bg-input)] hover:bg-[var(--bg-hover)]'
        }`}
      >
        <span
          className={`shrink-0 w-7 h-7 rounded-md flex items-center justify-center ${
            selected
              ? 'bg-[var(--accent)]/20 text-[var(--accent-hover)]'
              : 'bg-[var(--bg-elevated)] text-[var(--text-muted)]'
          }`}
        >
          {icon}
        </span>
        <span className="min-w-0 flex-1">
          <span className="block text-[12.5px] font-medium leading-tight text-[var(--text-primary)]">{title}</span>
          <span className="block mt-0.5 text-[11px] leading-tight text-[var(--text-dim)] truncate">{hint}</span>
        </span>
        <span
          className={`shrink-0 w-3.5 h-3.5 rounded-full border flex items-center justify-center ${
            selected ? 'border-[var(--accent)]' : 'border-[var(--text-dim)]/60'
          }`}
        >
          {selected && <span className="w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />}
        </span>
      </button>
    );
  };

  const callout = (tone: 'warn' | 'info', icon: React.ReactNode, children: React.ReactNode) => (
    <div
      className={`flex items-start gap-2 px-2.5 py-2 rounded-md border text-[11.5px] leading-snug ${
        tone === 'warn'
          ? 'border-[var(--modified-fg)]/30 bg-[var(--modified-fg)]/10 text-[var(--modified-fg)]'
          : 'border-[var(--border)] bg-[var(--bg-input)] text-[var(--text-muted)]'
      }`}
    >
      <span className="mt-[1px] shrink-0">{icon}</span>
      <span className="min-w-0">{children}</span>
    </div>
  );

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-[2px] p-4"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <form
        onSubmit={submit}
        role="dialog"
        aria-modal="true"
        aria-label="Publish to remote"
        className="w-full max-w-[460px] rounded-xl border border-[var(--border)] bg-[var(--bg-elevated)] shadow-[0_24px_64px_-12px_rgba(0,0,0,0.75)] overflow-hidden text-[var(--text-primary)]"
      >
        {/* Header */}
        <div className="flex items-center gap-3 px-5 pt-4 pb-3.5">
          <span className="shrink-0 w-9 h-9 rounded-lg flex items-center justify-center bg-[var(--accent)]/15 text-[var(--accent-hover)] border border-[var(--accent)]/25">
            <CloudUpload size={18} />
          </span>
          <div className="min-w-0 flex-1">
            <div className="text-[14px] font-semibold leading-tight">Publish to remote</div>
            <div className="mt-0.5 text-[11.5px] leading-tight text-[var(--text-muted)] truncate">
              {mode === 'github'
                ? gh?.login
                  ? `Create a repository on GitHub as ${gh.login}`
                  : 'Create a repository on GitHub'
                : 'Connect to a repository that already exists'}
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="shrink-0 p-1.5 -mr-1.5 rounded-md hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
            aria-label="Close"
          >
            <X size={15} />
          </button>
        </div>

        <div className="px-5 pb-5 space-y-4">
          <div
            role="tablist"
            className="flex gap-0.5 p-0.5 rounded-lg bg-[var(--bg-input)] border border-[var(--border)]"
          >
            {tab('github', 'GitHub', <Globe size={13} />)}
            {tab('remote', 'Existing remote URL', <Link2 size={13} />)}
          </div>

          {mode === 'github' ? (
            <>
              <div>
                <label className={labelCls} htmlFor="publish-repo-name">
                  Repository name
                </label>
                <div
                  className={`flex items-stretch h-[32px] rounded-md bg-[var(--bg-input)] border overflow-hidden transition-colors focus-within:border-[var(--accent)] focus-within:shadow-[0_0_0_3px_color-mix(in_srgb,var(--accent)_20%,transparent)] ${
                    name && !nameValid ? 'border-[var(--removed-fg)]' : 'border-[var(--border)]'
                  }`}
                >
                  <span className="shrink-0 max-w-[45%] truncate flex items-center px-2.5 text-[12.5px] text-[var(--text-dim)] bg-[var(--bg-elevated)]/60 border-r border-[var(--border)]">
                    {gh?.login || (ghLoading ? '…' : 'github.com')}
                    <span className="ml-1.5 text-[var(--text-dim)]/70">/</span>
                  </span>
                  <input
                    id="publish-repo-name"
                    value={name}
                    onChange={(e) => setName(e.target.value.replace(/\s+/g, '-'))}
                    autoFocus
                    spellCheck={false}
                    autoComplete="off"
                    className="min-w-0 flex-1 px-2.5 text-[12.5px] bg-transparent text-[var(--text-primary)] outline-none"
                  />
                </div>
                {name && !nameValid && (
                  <div className="mt-1.5 text-[11px] text-[var(--removed-fg)]">
                    Use letters, numbers, “.”, “-” and “_” only.
                  </div>
                )}
              </div>

              <div>
                <span className={labelCls}>Visibility</span>
                <div role="radiogroup" className="flex gap-2">
                  {visibilityCard(true, <Lock size={14} />, 'Private', 'Only you')}
                  {visibilityCard(false, <Globe size={14} />, 'Public', 'Anyone can see')}
                </div>
              </div>

              <div>
                <label className={labelCls} htmlFor="publish-repo-desc">
                  Description <span className="font-normal text-[var(--text-dim)]">(optional)</span>
                </label>
                <input
                  id="publish-repo-desc"
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  placeholder="What is this project about?"
                  autoComplete="off"
                  className={fieldCls}
                />
              </div>

              {ghLoading &&
                callout('info', <Loader2 size={13} className="animate-spin" />, 'Checking GitHub CLI…')}
              {!ghLoading && gh && !gh.installed &&
                callout(
                  'warn',
                  <AlertTriangle size={13} />,
                  <>
                    GitHub CLI (<span className="font-mono">gh</span>) isn’t installed. Install it from cli.github.com,
                    or use “Existing remote URL”.
                  </>
                )}
              {!ghLoading && gh && gh.installed && !gh.authenticated &&
                callout(
                  'warn',
                  <AlertTriangle size={13} />,
                  <>
                    Not signed in to GitHub. Run <span className="font-mono">gh auth login</span> in the terminal,
                    then reopen this dialog.
                  </>
                )}
              {ghReady && !isPrivate &&
                callout(
                  'warn',
                  <AlertTriangle size={13} />,
                  'Public repositories are visible to everyone on the internet.'
                )}
              {initNote && callout('info', <Info size={13} />, initNote)}
            </>
          ) : (
            <>
              <div>
                <label className={labelCls} htmlFor="publish-remote-url">
                  Remote URL
                </label>
                <input
                  id="publish-remote-url"
                  value={remoteUrl}
                  onChange={(e) => setRemoteUrl(e.target.value)}
                  placeholder="https://github.com/owner/repo.git"
                  autoFocus
                  spellCheck={false}
                  autoComplete="off"
                  className={`${fieldCls} font-mono text-[12px]`}
                />
              </div>
              {callout(
                'info',
                <Info size={13} />,
                <>
                  Added as <span className="font-mono">origin</span>
                  {isRepo ? '' : ' after initializing this folder as a Git repository'}. Use “Publish Branch”
                  afterwards to push.
                </>
              )}
            </>
          )}
        </div>

        {/* Footer */}
        <div className="flex items-center justify-end gap-2 px-5 py-3 border-t border-[var(--border)] bg-[var(--bg-sidebar)]/60">
          <button
            type="button"
            onClick={onClose}
            className="h-[30px] px-3.5 text-[12.5px] rounded-md border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={!canSubmit}
            className="h-[30px] px-4 text-[12.5px] font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:brightness-110 disabled:opacity-40 disabled:hover:brightness-100 cursor-pointer disabled:cursor-not-allowed flex items-center gap-1.5"
          >
            {busy ? <Loader2 size={13} className="animate-spin" /> : mode === 'github' ? <CloudUpload size={13} /> : <Link2 size={13} />}
            {mode === 'github' ? (busy ? 'Publishing…' : 'Publish Repository') : busy ? 'Adding…' : 'Add Remote'}
          </button>
        </div>
      </form>
    </div>
  );
};
