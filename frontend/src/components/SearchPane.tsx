import { MagnifyingGlass, ListMagnifyingGlass } from '@phosphor-icons/react';
import { EmptyState, EmptyStateButton } from './ui/EmptyState';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AtSign,
  Check,
  CheckCheck,
  CaseSensitive,
  ChevronDown,
  ChevronRight,
  Copy,
  FileCode,
  FoldVertical,
  GitCompare,
  Regex,
  RefreshCw,
  Search,
  SlidersHorizontal,
  UnfoldVertical,
  WholeWord,
  X,
} from 'lucide-react';
import { api } from '../api';
import type { GitFileStatus, GitStatusRes, SearchMatch, SearchRes } from '../types';
import { FileIcon } from './FileIcon';
import { fileStatusColor } from './GitBadges';
import { FloatingContextMenu, type ContextMenuItem } from './CommitContextMenu';
import { statusBadge } from './FileExplorer';

interface SearchPaneProps {
  active: boolean;
  gitStatus: GitStatusRes | null;
  /** Bumped whenever the backend reports filesystem changes. */
  fsVersion: number;
  onOpenFile: (path: string, line?: number, col?: number, colEnd?: number) => void;
  onOpenDiff: (path: string, staged?: boolean) => void;
  onAskAgent: (text: string) => void;
  onRefreshGit: () => Promise<void>;
  onToast: (msg: string, isError?: boolean) => void;
}

interface Group {
  path: string;
  name: string;
  dir: string;
  matches: SearchMatch[];
  count: number;
}

type Row =
  | { kind: 'file'; key: string; group: Group }
  | { kind: 'match'; key: string; group: Group; match: SearchMatch };

type MenuTarget = { x: number; y: number; group: Group; match?: SearchMatch };

const RECENT_KEY = 'vireo-recent-searches';
const headerBtn =
  'p-1.5 rounded-md hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer disabled:opacity-40 disabled:cursor-default disabled:hover:bg-transparent';
const fieldBox =
  'flex items-center h-[28px] pl-2 pr-1 gap-1 rounded-md bg-[var(--bg-input)] border border-[var(--border)] focus-within:border-[var(--accent)]';
const fieldInput =
  'flex-1 min-w-0 text-[12.5px] bg-transparent text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none';

const matchKey = (path: string, line: number) => `m:${path}:${line}`;
const fileKey = (path: string) => `f:${path}`;

function loadRecent(): string[] {
  try {
    const raw = localStorage.getItem(RECENT_KEY);
    const parsed = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((x) => typeof x === 'string').slice(0, 5) : [];
  } catch {
    return [];
  }
}

function saveRecent(list: string[]) {
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify(list));
  } catch {
    /* storage unavailable */
  }
}

const occurrences = (m: SearchMatch) => Math.max(1, m.ranges.length);

const ToggleBtn: React.FC<{
  active: boolean;
  onClick: () => void;
  title: string;
  children: React.ReactNode;
}> = ({ active, onClick, title, children }) => (
  <button
    type="button"
    onClick={onClick}
    title={title}
    aria-pressed={active}
    className={`w-5 h-5 shrink-0 flex items-center justify-center rounded cursor-pointer ${
      active
        ? 'bg-[var(--accent)]/20 text-[var(--accent-hover)] ring-1 ring-[var(--accent)]/50'
        : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
    }`}
  >
    {children}
  </button>
);

export const SearchPane: React.FC<SearchPaneProps> = ({
  active,
  gitStatus,
  fsVersion,
  onOpenFile,
  onOpenDiff,
  onAskAgent,
  onRefreshGit,
  onToast,
}) => {
  const [query, setQuery] = useState('');
  const [replaceText, setReplaceText] = useState('');
  const [showReplace, setShowReplace] = useState(false);
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [wholeWord, setWholeWord] = useState(false);
  const [useRegex, setUseRegex] = useState(false);
  const [scope, setScope] = useState<'all' | 'changed'>('all');
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [include, setInclude] = useState('');
  const [exclude, setExclude] = useState('');
  const [useGitignore, setUseGitignore] = useState(true);

  const [results, setResults] = useState<SearchRes | null>(null);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshTick, setRefreshTick] = useState(0);

  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [dismissedFiles, setDismissedFiles] = useState<Set<string>>(new Set());
  const [dismissedLines, setDismissedLines] = useState<Set<string>>(new Set());
  const [activeKey, setActiveKey] = useState<string | null>(null);
  const [menu, setMenu] = useState<MenuTarget | null>(null);
  const [confirmAll, setConfirmAll] = useState(false);
  const [replacing, setReplacing] = useState(false);
  const [recent, setRecent] = useState<string[]>(loadRecent);

  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const lastKeyRef = useRef('');

  // Git state --------------------------------------------------------------
  const statusByPath = useMemo(() => {
    const map = new Map<string, { badge: string; file: GitFileStatus }>();
    for (const f of gitStatus?.files ?? []) map.set(f.path, { badge: statusBadge(f), file: f });
    return map;
  }, [gitStatus]);

  const changedPaths = useMemo(
    () =>
      (gitStatus?.files ?? [])
        .filter((f) => statusBadge(f) !== 'D')
        .map((f) => f.path)
        .sort(),
    [gitStatus]
  );
  const changedKey = changedPaths.join('\n');
  const totalChanged = gitStatus?.files.length ?? 0;
  const filtersActive = include.trim() !== '' || exclude.trim() !== '' || !useGitignore;
  const anyFilter = filtersActive || caseSensitive || wholeWord || scope === 'changed';

  // Focus the query box whenever the pane is shown (Ctrl+Shift+F, tab click).
  useEffect(() => {
    if (active) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [active]);

  // Debounced live search --------------------------------------------------
  const searchKey = JSON.stringify([
    query,
    caseSensitive,
    wholeWord,
    useRegex,
    include,
    exclude,
    useGitignore,
    scope,
    scope === 'changed' ? changedKey : '',
  ]);

  useEffect(() => {
    if (!query) {
      setResults(null);
      setError(null);
      setSearching(false);
      lastKeyRef.current = searchKey;
      return;
    }
    if (!active) return;

    const ctrl = new AbortController();
    const run = async () => {
      setSearching(true);
      try {
        const res = await api.searchFiles(
          {
            q: query,
            case_sensitive: caseSensitive,
            whole_word: wholeWord,
            regex: useRegex,
            include,
            exclude,
            use_gitignore: useGitignore,
            only_paths: scope === 'changed' ? changedPaths : null,
          },
          ctrl.signal
        );
        if (ctrl.signal.aborted) return;
        if (lastKeyRef.current !== searchKey) {
          lastKeyRef.current = searchKey;
          setDismissedFiles(new Set());
          setDismissedLines(new Set());
          setCollapsed(new Set());
          setActiveKey(null);
        }
        setResults(res);
        setError(null);
      } catch (e) {
        if (ctrl.signal.aborted) return;
        setResults(null);
        setError((e as Error).message || 'Search failed');
      } finally {
        if (!ctrl.signal.aborted) setSearching(false);
      }
    };
    const t = setTimeout(run, 250);
    return () => {
      clearTimeout(t);
      ctrl.abort();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchKey, refreshTick, fsVersion, active]);

  // Grouped, filtered view of the results ------------------------------------
  const groups = useMemo<Group[]>(() => {
    if (!results) return [];
    const byPath = new Map<string, Group>();
    for (const m of results.matches) {
      if (dismissedFiles.has(m.path) || dismissedLines.has(matchKey(m.path, m.line))) continue;
      let g = byPath.get(m.path);
      if (!g) {
        const slash = m.path.lastIndexOf('/');
        g = {
          path: m.path,
          name: m.path.slice(slash + 1),
          dir: slash >= 0 ? m.path.slice(0, slash) : '',
          matches: [],
          count: 0,
        };
        byPath.set(m.path, g);
      }
      g.matches.push(m);
      g.count += occurrences(m);
    }
    return [...byPath.values()];
  }, [results, dismissedFiles, dismissedLines]);

  const totalCount = useMemo(() => groups.reduce((n, g) => n + g.count, 0), [groups]);

  const rows = useMemo<Row[]>(() => {
    const out: Row[] = [];
    for (const g of groups) {
      out.push({ kind: 'file', key: fileKey(g.path), group: g });
      if (collapsed.has(g.path)) continue;
      for (const m of g.matches) {
        out.push({ kind: 'match', key: matchKey(g.path, m.line), group: g, match: m });
      }
    }
    return out;
  }, [groups, collapsed]);

  useEffect(() => {
    if (!activeKey || !listRef.current) return;
    const el = listRef.current.querySelector(`[data-row-key="${CSS.escape(activeKey)}"]`);
    el?.scrollIntoView({ block: 'nearest' });
  }, [activeKey]);

  // Replace preview -----------------------------------------------------------
  const previewRegex = useMemo(() => {
    if (!useRegex || !query) return null;
    try {
      const src = wholeWord ? `\\b(?:${query})\\b` : query;
      return new RegExp(src, caseSensitive ? '' : 'i');
    } catch {
      return null;
    }
  }, [useRegex, query, wholeWord, caseSensitive]);

  const previewReplacement = useCallback(
    (matched: string) => {
      if (!previewRegex) return replaceText;
      const m = previewRegex.exec(matched);
      if (!m) return replaceText;
      // Mirror the Rust regex crate's `$name` / `${name}` / `$$` expansion rules.
      return replaceText.replace(
        /\$(\$|\{(\w+)\}|(\w+))/g,
        (_all, dollar: string, braced?: string, bare?: string) => {
          if (dollar === '$') return '$';
          const name = (braced ?? bare) as string;
          const v = /^\d+$/.test(name) ? m[Number(name)] : m.groups?.[name];
          return v ?? '';
        }
      );
    },
    [previewRegex, replaceText]
  );

  const showPreview = showReplace && replaceText !== '';

  // Actions ---------------------------------------------------------------------
  const rememberQuery = () => {
    const q = query.trim();
    if (q.length < 2) return;
    const next = [q, ...recent.filter((r) => r !== q)].slice(0, 5);
    setRecent(next);
    saveRecent(next);
  };

  const openMatch = (g: Group, m: SearchMatch) => {
    rememberQuery();
    const r = m.ranges[0];
    onOpenFile(g.path, m.line, r ? r[0] + 1 : undefined, r ? r[1] + 1 : undefined);
  };

  const toggleCollapse = (path: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const dismissFile = (path: string) => setDismissedFiles((prev) => new Set(prev).add(path));
  const dismissLine = (path: string, line: number) =>
    setDismissedLines((prev) => new Set(prev).add(matchKey(path, line)));

  const clearAll = () => {
    setQuery('');
    setReplaceText('');
    setConfirmAll(false);
    inputRef.current?.focus();
  };

  const resetFilters = () => {
    setScope('all');
    setCaseSensitive(false);
    setWholeWord(false);
    setInclude('');
    setExclude('');
    setUseGitignore(true);
  };

  const copyText = async (text: string, label: string) => {
    try {
      await navigator.clipboard.writeText(text);
      onToast(`Copied ${label}`);
    } catch {
      onToast('Copy failed', true);
    }
  };

  const runReplace = async (targets: { path: string; lines: number[] }[], label: string) => {
    if (targets.length === 0 || replacing) return;
    setReplacing(true);
    try {
      const res = await api.replaceInFiles({
        q: query,
        case_sensitive: caseSensitive,
        whole_word: wholeWord,
        regex: useRegex,
        replacement: replaceText,
        targets,
      });
      if (res.skipped.length > 0) {
        onToast(
          `Replaced ${res.replacements} in ${res.files_changed} file(s); skipped ${res.skipped.length}`,
          true
        );
      } else {
        onToast(`Replaced ${res.replacements} match${res.replacements === 1 ? '' : 'es'} ${label}`);
      }
      setRefreshTick((n) => n + 1);
      void onRefreshGit();
    } catch (e) {
      onToast((e as Error).message || 'Replace failed', true);
    } finally {
      setReplacing(false);
      setConfirmAll(false);
    }
  };

  const replaceLine = (g: Group, m: SearchMatch) =>
    runReplace([{ path: g.path, lines: [m.line] }], `in ${g.name}`);
  const replaceFile = (g: Group) =>
    runReplace([{ path: g.path, lines: g.matches.map((m) => m.line) }], `in ${g.name}`);
  const replaceEverything = () =>
    runReplace(
      groups.map((g) => ({ path: g.path, lines: g.matches.map((m) => m.line) })),
      `in ${groups.length} file${groups.length === 1 ? '' : 's'}`
    );

  const mentionFile = (g: Group) => {
    const lines = g.matches.map((m) => m.line);
    const shown = lines.slice(0, 8).join(', ');
    onAskAgent(`@${g.path} (lines ${shown}${lines.length > 8 ? ', …' : ''})`);
  };
  const mentionLine = (g: Group, m: SearchMatch) =>
    onAskAgent(`@${g.path}:L${m.line} \`${m.text.trim()}\``);

  const sendList = () => {
    const lines: string[] = [];
    let n = 0;
    outer: for (const g of groups) {
      for (const m of g.matches) {
        if (n >= 40) break outer;
        lines.push(`- @${g.path}:L${m.line} \`${m.text.trim().slice(0, 120)}\``);
        n++;
      }
    }
    const rest = groups.reduce((a, g) => a + g.matches.length, 0) - n;
    if (rest > 0) lines.push(`- …and ${rest} more`);
    onAskAgent(
      `Search results for \`${query}\` (${totalCount} match${totalCount === 1 ? '' : 'es'} in ${groups.length} file${groups.length === 1 ? '' : 's'}):\n${lines.join('\n')}`
    );
  };

  // Context menu ------------------------------------------------------------------
  const buildMenu = (t: MenuTarget): ContextMenuItem[] => {
    const { group: g, match: m } = t;
    const st = statusByPath.get(g.path);
    const canReplace = showReplace && replaceText !== '';
    const items: ContextMenuItem[] = [
      {
        id: 'open',
        label: m ? 'Open File at Line' : 'Open File',
        icon: <FileCode size={13} />,
        disabled: st?.badge === 'D',
        onClick: () => (m ? openMatch(g, m) : onOpenFile(g.path)),
      },
      {
        id: 'diff',
        label: 'Open Changes',
        icon: <GitCompare size={13} />,
        disabled: !st,
        onClick: () => onOpenDiff(g.path, !!st && st.file.staged && !st.file.unstaged),
      },
      {
        id: 'agent',
        label: 'Mention in Agent Chat',
        icon: <AtSign size={13} />,
        dividerBefore: true,
        onClick: () => (m ? mentionLine(g, m) : mentionFile(g)),
      },
    ];
    if (showReplace) {
      items.push({
        id: 'replace',
        label: m ? 'Replace Line' : 'Replace in File',
        icon: <Check size={13} />,
        dividerBefore: true,
        disabled: !canReplace,
        onClick: () => (m ? replaceLine(g, m) : replaceFile(g)),
      });
    }
    if (m) {
      items.push({
        id: 'copy-line',
        label: 'Copy Matching Line',
        icon: <Copy size={13} />,
        dividerBefore: true,
        onClick: () => copyText(m.text.trim(), 'line'),
      });
      items.push({
        id: 'copy-loc',
        label: 'Copy path:line',
        icon: <Copy size={13} />,
        onClick: () => copyText(`${g.path}:${m.line}`, `${g.path}:${m.line}`),
      });
    } else {
      items.push({
        id: 'copy-path',
        label: 'Copy Relative Path',
        icon: <Copy size={13} />,
        dividerBefore: true,
        onClick: () => copyText(g.path, 'path'),
      });
    }
    items.push({
      id: 'dismiss',
      label: 'Dismiss from Results',
      icon: <X size={13} />,
      dividerBefore: true,
      onClick: () => (m ? dismissLine(g.path, m.line) : dismissFile(g.path)),
    });
    return items;
  };

  // Keyboard navigation -------------------------------------------------------------
  const onListKey = (e: React.KeyboardEvent) => {
    if (rows.length === 0) return;
    const idx = rows.findIndex((r) => r.key === activeKey);
    const cur = idx >= 0 ? rows[idx] : null;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setActiveKey(rows[Math.min(idx + 1, rows.length - 1)].key);
        break;
      case 'ArrowUp':
        e.preventDefault();
        if (idx <= 0) inputRef.current?.focus();
        else setActiveKey(rows[idx - 1].key);
        break;
      case 'Enter':
        if (!cur) break;
        e.preventDefault();
        if (cur.kind === 'file') toggleCollapse(cur.group.path);
        else openMatch(cur.group, cur.match);
        break;
      case 'ArrowLeft':
        if (!cur) break;
        e.preventDefault();
        if (cur.kind === 'match') setActiveKey(fileKey(cur.group.path));
        else if (!collapsed.has(cur.group.path)) toggleCollapse(cur.group.path);
        break;
      case 'ArrowRight':
        if (cur?.kind === 'file' && collapsed.has(cur.group.path)) {
          e.preventDefault();
          toggleCollapse(cur.group.path);
        }
        break;
      case 'Delete':
      case 'Backspace': {
        if (!cur) break;
        e.preventDefault();
        const next = rows[idx + 1] ?? rows[idx - 1];
        setActiveKey(next && next.key !== cur.key ? next.key : null);
        if (cur.kind === 'file') dismissFile(cur.group.path);
        else dismissLine(cur.group.path, cur.match.line);
        break;
      }
    }
  };

  const onInputKey = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown' && rows.length > 0) {
      e.preventDefault();
      setActiveKey((k) => k ?? rows[0].key);
      listRef.current?.focus();
    } else if (e.key === 'Enter') {
      rememberQuery();
      setRefreshTick((n) => n + 1);
    } else if (e.key === 'Escape' && query) {
      e.preventDefault();
      setQuery('');
    }
  };

  // Rendering -------------------------------------------------------------------------
  const renderSnippet = (m: SearchMatch) => {
    const text = m.text;
    const lead = text.length - text.trimStart().length;
    const ranges = m.ranges
      .map(([s, e]) => [Math.max(0, s - lead), e - lead] as const)
      .filter(([, e]) => e > 0);
    const body = text.slice(lead);
    let start = 0;
    let prefix = false;
    if (ranges.length > 0 && ranges[0][0] > 20) {
      start = ranges[0][0] - 12;
      prefix = true;
    }
    const parts: React.ReactNode[] = [];
    let cursor = start;
    ranges.forEach(([s, e], i) => {
      if (e <= cursor) return;
      const from = Math.max(s, cursor);
      if (from > cursor) parts.push(body.slice(cursor, from));
      const matched = body.slice(from, e);
      if (showPreview) {
        parts.push(
          <span
            key={`r${i}`}
            className="bg-[var(--removed-bg)] text-[var(--removed-fg)] line-through px-[1px] rounded-[2px]"
          >
            {matched}
          </span>,
          <span
            key={`a${i}`}
            className="bg-[var(--added-bg)] text-[var(--added-fg)] font-semibold px-[1px] rounded-[2px] ml-0.5"
          >
            {previewReplacement(matched)}
          </span>
        );
      } else {
        parts.push(
          <mark
            key={`m${i}`}
            className="bg-[var(--accent)]/30 text-[var(--text-primary)] rounded-[3px]"
          >
            {matched}
          </mark>
        );
      }
      cursor = e;
    });
    parts.push(body.slice(cursor));
    return (
      <>
        {prefix && <span className="text-[var(--text-dim)]">…</span>}
        {parts.map((p, i) => (typeof p === 'string' ? <React.Fragment key={i}>{p}</React.Fragment> : p))}
      </>
    );
  };

  const renderFileRow = (g: Group) => {
    const key = fileKey(g.path);
    const st = statusByPath.get(g.path);
    const isCollapsed = collapsed.has(g.path);
    const isActive = activeKey === key;
    const canReplace = showReplace && replaceText !== '';
    const act = 'p-1 rounded hover:bg-[var(--bg-active)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer disabled:opacity-40 disabled:cursor-default';
    return (
      <div
        key={key}
        data-row-key={key}
        onClick={() => {
          setActiveKey(key);
          toggleCollapse(g.path);
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          setActiveKey(key);
          setMenu({ x: e.clientX, y: e.clientY, group: g });
        }}
        className={`group relative mx-1.5 mt-0.5 first:mt-0 flex items-center gap-1.5 h-[26px] pl-1 pr-2 rounded-md text-[12.5px] cursor-pointer select-none ${
          isActive ? 'bg-[var(--bg-active)]' : 'hover:bg-[var(--bg-hover)]'
        }`}
        title={g.path}
      >
        <ChevronRight
          size={13}
          className={`shrink-0 text-[var(--text-dim)] transition-transform duration-100 ${
            isCollapsed ? '' : 'rotate-90'
          }`}
        />
        <FileIcon path={g.path} size={15} />
        <span className="flex-1 min-w-0 flex items-baseline gap-1.5 overflow-hidden">
          <span
            className="font-medium truncate min-w-[3ch] shrink"
            style={{ color: st ? fileStatusColor(st.badge) : 'var(--text-primary)' }}
          >
            {g.name}
          </span>
          {g.dir && (
            <span className="truncate shrink-[3] text-[11px] text-[var(--text-dim)]">{g.dir}</span>
          )}
        </span>
        <div className="flex items-center gap-1.5 shrink-0 group-hover:hidden">
          {st && (
            <span
              className="font-mono text-[10.5px] font-bold"
              style={{ color: fileStatusColor(st.badge) }}
            >
              {st.badge}
            </span>
          )}
          <span className="min-w-[18px] h-[16px] px-1.5 flex items-center justify-center rounded-full bg-[var(--bg-elevated)] text-[var(--text-muted)] text-[10px] font-medium tabular-nums">
            {g.count}
          </span>
        </div>
        <div className="hidden group-hover:flex items-center gap-0.5 shrink-0">
          {showReplace && (
            <button
              className={act}
              disabled={!canReplace || replacing}
              title={canReplace ? 'Replace in file' : 'Type a replacement first'}
              onClick={(e) => {
                e.stopPropagation();
                replaceFile(g);
              }}
            >
              <CheckCheck size={12.5} />
            </button>
          )}
          {st && (
            <button
              className={act}
              title="Open changes"
              onClick={(e) => {
                e.stopPropagation();
                onOpenDiff(g.path, st.file.staged && !st.file.unstaged);
              }}
            >
              <GitCompare size={12.5} />
            </button>
          )}
          <button
            className={act}
            title="Mention in Agent"
            onClick={(e) => {
              e.stopPropagation();
              mentionFile(g);
            }}
          >
            <AtSign size={12.5} />
          </button>
          <button
            className={act}
            title="Dismiss file"
            onClick={(e) => {
              e.stopPropagation();
              dismissFile(g.path);
            }}
          >
            <X size={12.5} />
          </button>
        </div>
      </div>
    );
  };

  const renderMatchRow = (g: Group, m: SearchMatch) => {
    const key = matchKey(g.path, m.line);
    const isActive = activeKey === key;
    const canReplace = showReplace && replaceText !== '';
    const bg = isActive ? 'var(--bg-active)' : 'var(--bg-hover)';
    const act = 'p-1 rounded hover:bg-[var(--bg-active)] text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer disabled:opacity-40 disabled:cursor-default';
    return (
      <div
        key={key}
        data-row-key={key}
        onClick={() => {
          setActiveKey(key);
          openMatch(g, m);
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          setActiveKey(key);
          setMenu({ x: e.clientX, y: e.clientY, group: g, match: m });
        }}
        className={`group relative mx-1.5 flex items-center gap-2 h-[22px] pl-[26px] pr-2 rounded-md text-[12px] cursor-pointer ${
          isActive ? 'bg-[var(--bg-active)]' : 'hover:bg-[var(--bg-hover)]'
        }`}
      >
        <span className="absolute left-[10px] top-0 bottom-0 w-px bg-[var(--border)]" />
        <span className="min-w-[3ch] shrink-0 text-right font-mono text-[10.5px] text-[var(--text-dim)] tabular-nums">
          {m.line}
        </span>
        <span className="truncate flex-1 min-w-0 whitespace-pre text-[var(--text-muted)]">
          {renderSnippet(m)}
        </span>
        <div
          className="absolute right-1 top-0 bottom-0 hidden group-hover:flex items-center gap-0.5 pl-6"
          style={{ background: `linear-gradient(to left, ${bg} 70%, transparent)` }}
        >
          {showReplace && (
            <button
              className={act}
              disabled={!canReplace || replacing}
              title={canReplace ? 'Replace line' : 'Type a replacement first'}
              onClick={(e) => {
                e.stopPropagation();
                replaceLine(g, m);
              }}
            >
              <Check size={12} />
            </button>
          )}
          <button
            className={act}
            title="Send line to Agent"
            onClick={(e) => {
              e.stopPropagation();
              mentionLine(g, m);
            }}
          >
            <AtSign size={12} />
          </button>
          <button
            className={act}
            title="Dismiss match"
            onClick={(e) => {
              e.stopPropagation();
              dismissLine(g.path, m.line);
            }}
          >
            <X size={12} />
          </button>
        </div>
      </div>
    );
  };

  const hasQuery = query !== '';
  const showSummary = hasQuery && (searching || results || error);
  const anyExpanded = groups.some((g) => !collapsed.has(g.path));

  return (
    <div className="flex-1 flex flex-col min-h-0">
      {/* Zone 1: header */}
      <div className="flex items-center justify-between h-10 px-3.5 border-b border-[var(--border)] shrink-0">
        <span className="text-[11.5px] font-semibold uppercase tracking-wider text-[var(--text-muted)]">
          Search
        </span>
        <div className="flex items-center gap-0.5">
          <button
            onClick={() =>
              setCollapsed(anyExpanded ? new Set(groups.map((g) => g.path)) : new Set())
            }
            disabled={groups.length === 0}
            className={headerBtn}
            title={anyExpanded ? 'Collapse all' : 'Expand all'}
          >
            {anyExpanded ? <FoldVertical size={14} /> : <UnfoldVertical size={14} />}
          </button>
          <button
            onClick={clearAll}
            disabled={!hasQuery && !replaceText}
            className={headerBtn}
            title="Clear search"
          >
            <X size={14} />
          </button>
          <button
            onClick={() => setRefreshTick((n) => n + 1)}
            disabled={!hasQuery}
            className={headerBtn}
            title="Re-run search"
          >
            <RefreshCw size={13.5} className={searching ? 'animate-spin' : ''} />
          </button>
        </div>
      </div>

      {/* Zone 2: query, replace and scope */}
      <div className="px-2.5 pt-2 pb-2 border-b border-[var(--border)] shrink-0">
        <div className="flex gap-1">
          <button
            type="button"
            onClick={() => setShowReplace((v) => !v)}
            className="w-4 shrink-0 flex items-center justify-center rounded text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
            title={showReplace ? 'Hide replace' : 'Toggle replace'}
          >
            {showReplace ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          </button>
          <div className="flex-1 min-w-0 flex flex-col gap-1.5">
            <div className={fieldBox}>
              <Search size={12.5} className="shrink-0 text-[var(--text-dim)]" />
              <input
                ref={inputRef}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={onInputKey}
                placeholder="Search"
                spellCheck={false}
                className={fieldInput}
              />
              {hasQuery && (
                <button
                  type="button"
                  onClick={clearAll}
                  title="Clear"
                  className="w-5 h-5 shrink-0 flex items-center justify-center rounded text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
                >
                  <X size={12} />
                </button>
              )}
              <ToggleBtn active={caseSensitive} onClick={() => setCaseSensitive((v) => !v)} title="Match case">
                <CaseSensitive size={14} />
              </ToggleBtn>
              <ToggleBtn active={wholeWord} onClick={() => setWholeWord((v) => !v)} title="Match whole word">
                <WholeWord size={14} />
              </ToggleBtn>
              <ToggleBtn active={useRegex} onClick={() => setUseRegex((v) => !v)} title="Use regular expression">
                <Regex size={14} />
              </ToggleBtn>
            </div>

            {showReplace && (
              <div className="flex items-center gap-1">
                <div className={`${fieldBox} flex-1 min-w-0`}>
                  <input
                    value={replaceText}
                    onChange={(e) => {
                      setReplaceText(e.target.value);
                      setConfirmAll(false);
                    }}
                    placeholder="Replace"
                    spellCheck={false}
                    className={fieldInput}
                  />
                </div>
                <button
                  type="button"
                  onClick={() => setConfirmAll(true)}
                  disabled={groups.length === 0 || replacing}
                  title="Replace all"
                  className="h-[28px] px-2 shrink-0 flex items-center gap-1 rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[11.5px] font-medium text-[var(--text-primary)] cursor-pointer disabled:opacity-40 disabled:cursor-default"
                >
                  <CheckCheck size={13} />
                  All
                </button>
              </div>
            )}

            {showReplace && confirmAll && (
              <div className="rounded-md border border-[var(--modified-fg)]/40 bg-[var(--bg-elevated)] px-2 py-1.5 text-[11.5px] text-[var(--text-muted)]">
                <div>
                  {replaceText === '' ? 'Delete' : 'Replace'}{' '}
                  <span className="font-semibold text-[var(--text-primary)] tabular-nums">{totalCount}</span>{' '}
                  match{totalCount === 1 ? '' : 'es'} in{' '}
                  <span className="font-semibold text-[var(--text-primary)] tabular-nums">{groups.length}</span>{' '}
                  file{groups.length === 1 ? '' : 's'}? This writes to disk.
                  {results?.truncated && (
                    <span className="text-[var(--modified-fg)]"> Results are capped; some matches are not included.</span>
                  )}
                </div>
                <div className="mt-1.5 flex gap-1.5">
                  <button
                    onClick={replaceEverything}
                    disabled={replacing}
                    className="h-6 px-2 rounded-md bg-[var(--accent)] text-[var(--accent-fg)] text-[11.5px] font-medium cursor-pointer disabled:opacity-50"
                  >
                    {replacing ? 'Replacing…' : 'Replace'}
                  </button>
                  <button
                    onClick={() => setConfirmAll(false)}
                    className="h-6 px-2 rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] text-[11.5px] cursor-pointer"
                  >
                    Cancel
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>

        <div className="mt-2 pl-5 flex items-center justify-between gap-1">
          <div className="flex items-center p-0.5 rounded-md bg-[var(--bg-input)] border border-[var(--border)]">
            {(['all', 'changed'] as const).map((s) => {
              const disabled = s === 'changed' && totalChanged === 0;
              return (
                <button
                  key={s}
                  type="button"
                  disabled={disabled}
                  onClick={() => setScope(s)}
                  className={`h-[20px] px-2 rounded-[4px] text-[11px] whitespace-nowrap cursor-pointer disabled:opacity-40 disabled:cursor-default ${
                    scope === s
                      ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium shadow-sm'
                      : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                  }`}
                >
                  {s === 'all' ? 'All files' : `Changed (${totalChanged})`}
                </button>
              );
            })}
          </div>
          <button
            type="button"
            onClick={() => setFiltersOpen((v) => !v)}
            className={`relative h-[22px] px-1.5 flex items-center gap-1 rounded-md text-[11px] cursor-pointer ${
              filtersOpen
                ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
            }`}
            title="Include / exclude filters"
          >
            <SlidersHorizontal size={12} />
            Filters
            {filtersActive && (
              <span className="w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
            )}
          </button>
        </div>

        {filtersOpen && (
          <div className="mt-1.5 pl-5 flex flex-col gap-1.5">
            <input
              value={include}
              onChange={(e) => setInclude(e.target.value)}
              placeholder="Include, e.g. frontend/**, *.rs"
              spellCheck={false}
              className={`${fieldBox} h-[24px] font-mono text-[11.5px] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none`}
            />
            <input
              value={exclude}
              onChange={(e) => setExclude(e.target.value)}
              placeholder="Exclude, e.g. *.lock, dist/**"
              spellCheck={false}
              className={`${fieldBox} h-[24px] font-mono text-[11.5px] text-[var(--text-primary)] placeholder:text-[var(--text-dim)] outline-none`}
            />
            <label className="flex items-center gap-1.5 text-[11px] text-[var(--text-muted)] cursor-pointer select-none">
              <input
                type="checkbox"
                checked={useGitignore}
                onChange={(e) => setUseGitignore(e.target.checked)}
                className="accent-[var(--accent)]"
              />
              Respect .gitignore
            </label>
          </div>
        )}
      </div>

      {/* Zone 3: summary */}
      {showSummary && (
        <div className="flex items-center gap-2 h-7 pl-3.5 pr-2 shrink-0 text-[11px] whitespace-nowrap">
          <span className="min-w-0 truncate text-[var(--text-muted)] tabular-nums">
            {searching && !results ? (
              'Searching…'
            ) : error ? (
              ''
            ) : (
              <>
                <span className="font-medium text-[var(--text-primary)]">
                  {totalCount.toLocaleString()}
                  {results?.truncated ? '+' : ''}
                </span>{' '}
                result{totalCount === 1 ? '' : 's'} in{' '}
                <span className="font-medium text-[var(--text-primary)]">{groups.length}</span>{' '}
                file{groups.length === 1 ? '' : 's'}
              </>
            )}
          </span>
          {results?.truncated && !searching && !error && (
            <span
              className="shrink-0 px-1.5 h-[16px] leading-[16px] rounded bg-[var(--modified-fg)]/12 text-[var(--modified-fg)] text-[10px] font-medium"
              title="Too many matches, only the first results are shown. Refine the query to narrow them down."
            >
              Capped
            </span>
          )}
          {groups.length > 0 && (
            <button
              onClick={sendList}
              className="ml-auto shrink-0 flex items-center gap-1 h-5 px-1.5 rounded text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
              title="Send these results to the Agent"
            >
              <AtSign size={11} />
              Ask agent
            </button>
          )}
        </div>
      )}

      {/* Zone 4: results */}
      <div
        ref={listRef}
        tabIndex={0}
        onKeyDown={onListKey}
        className="flex-1 overflow-y-auto py-1 outline-none"
      >
        {!hasQuery ? (
          <div className="text-center text-[12px] text-[var(--text-dim)]">
            <EmptyState
              icon={MagnifyingGlass}
              title="Search the workspace"
              description="Find text across every file. Use the filters to narrow by path or file type."
              className={recent.length > 0 ? '!pb-3' : ''}
            />
            {recent.length > 0 && (
              <div className="px-4 pb-4 space-y-1.5">
                <div className="flex items-center justify-center gap-2 text-[10.5px] uppercase tracking-wider">
                  Recent
                  <button
                    onClick={() => {
                      setRecent([]);
                      saveRecent([]);
                    }}
                    className="normal-case tracking-normal hover:text-[var(--text-primary)] cursor-pointer"
                  >
                    clear
                  </button>
                </div>
                <div className="flex flex-wrap justify-center gap-1.5">
                  {recent.map((r) => (
                    <button
                      key={r}
                      onClick={() => setQuery(r)}
                      className="flex items-center gap-1 max-w-full px-2 h-[22px] rounded-full bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[11.5px] font-mono text-[var(--text-muted)] hover:text-[var(--text-primary)] cursor-pointer"
                    >
                      <Search size={10.5} className="shrink-0" />
                      <span className="truncate">{r}</span>
                    </button>
                  ))}
                </div>
              </div>
            )}
          </div>
        ) : error ? (
          <div className="px-4 py-4 text-[12px] text-[var(--removed-fg)] break-words">{error}</div>
        ) : searching && !results ? (
          <div className="px-4 py-4 text-[12px] text-[var(--text-muted)]">Searching…</div>
        ) : results && groups.length === 0 ? (
          <EmptyState
            icon={ListMagnifyingGlass}
            title={results.matches.length > 0 ? 'All results dismissed' : 'No results'}
            description={
              results.matches.length > 0 ? (
                <>
                  Every match for <span className="text-[var(--text-muted)] break-all">“{query}”</span> was dismissed.
                </>
              ) : (
                <>
                  Nothing matches <span className="text-[var(--text-muted)] break-all">“{query}”</span>
                  {scope === 'changed' ? ` in ${totalChanged} changed file${totalChanged === 1 ? '' : 's'}` : ''}.
                </>
              )
            }
            action={
              anyFilter && (
                <EmptyStateButton onClick={resetFilters}>
                  {scope === 'changed' ? 'Search all files' : 'Reset filters'}
                </EmptyStateButton>
              )
            }
          />
        ) : (
          rows.map((r) => (r.kind === 'file' ? renderFileRow(r.group) : renderMatchRow(r.group, r.match)))
        )}
      </div>

      {menu && (
        <FloatingContextMenu
          x={menu.x}
          y={menu.y}
          items={buildMenu(menu)}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
};
