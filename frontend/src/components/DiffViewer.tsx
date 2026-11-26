import { Binary, Equals, GitDiff } from '@phosphor-icons/react';
import { EmptyState, EmptyStateButton } from './ui/EmptyState';
import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { diffLines } from 'diff';
import { computeChangedRanges, type CharRange } from '../utils/wordDiff';
import {
  ArrowLeft,
  ArrowRight,
  Check,
  ChevronDown,
  ChevronUp,
  Columns2,
  Copy,
  Edit3,
  FoldVertical,
  GitCommitHorizontal,
  MessageSquarePlus,
  Minus,
  Plus,
  RotateCcw,
  Rows3,
  Space,
  UnfoldVertical,
  WrapText,
} from 'lucide-react';
import { highlightLines, type TokenSpan } from '../syntax';
import type { LiteTheme } from '../themes';
import type { GitDiffRes, GitFileStatus } from '../types';

export interface DiffCommitContext {
  mode: 'commit' | 'compare';
  hash: string;
  shortHash: string;
  subject: string;
  author?: string;
  date?: string;
}

export interface DiffFileNav {
  index: number;
  total: number;
  onPrev: () => void;
  onNext: () => void;
}

interface DiffViewerProps {
  diff: GitDiffRes | null;
  fileStatus?: GitFileStatus;
  commitContext?: DiffCommitContext | null;
  fileNav?: DiffFileNav | null;
  theme: LiteTheme;
  loading: boolean;
  onStage?: () => void;
  onUnstage?: () => void;
  onDiscard?: () => void;
  onApplyHunk?: (patch: string, action: 'stage' | 'unstage' | 'revert') => Promise<void>;
  onSwitchToCode: () => void;
  onAskAgent: (snippet: string) => void;
  onToast?: (msg: string, isError?: boolean) => void;
  /** Lines just changed on disk (by an agent / external tool); glow + scroll to them. */
  liveFlash?: LiveFlash | null;
  /** Scroll to live edits automatically (paused while the user is interacting). */
  followEdits?: boolean;
  /** Timestamp (ms) of the user's last scroll / key / pointer activity. */
  userActivityRef?: React.MutableRefObject<number>;
}

interface UnifiedRow {
  type: 'context' | 'add' | 'remove';
  oldLine: number | null;
  newLine: number | null;
  text: string;
  pairedText?: string;
  cluster: number;
}

interface SplitRow {
  left: { line: number | null; text: string; type: 'context' | 'remove' | 'empty' };
  right: { line: number | null; text: string; type: 'context' | 'add' | 'empty' };
  pairHighlight?: boolean;
  cluster: number;
}

/** A cluster of nearby changes shown (and staged / reverted) as one unit. */
interface HunkMeta {
  index: number;
  oldStart: number;
  oldCount: number;
  newStart: number;
  newCount: number;
  patch: string;
  /** Unified-row range of this cluster (inclusive). */
  firstRow: number;
  lastRow: number;
  /** Text of the whole cluster as a diff, for "Ask". */
  excerpt: string;
}

type FoldedItem<T> =
  | { kind: 'row'; row: T; index: number; cluster: number; clusterStart: boolean }
  | { kind: 'fold'; id: string; startIndex: number; endIndex: number; count: number };

export interface LiveFlash {
  key: number;
  /** 1-based inclusive line ranges on the new (right) side. */
  ranges: Array<{ from: number; to: number }>;
  /** New-side line the edit is at; locates pure deletions, which have no added lines. */
  anchor?: number;
  /** Scroll even when following is off or the user is busy (they asked to see this edit). */
  reveal?: boolean;
}

const CONTEXT_LINES = 3;
const MIN_FOLD_LINES = 4;
/** Changes whose unchanged gap is below this stay in one visual cluster (no fold between them). */
const CLUSTER_GAP = 2 * CONTEXT_LINES + MIN_FOLD_LINES;
const EXPAND_STEP = 12;

/** Assign a cluster id to every row from first to last change of a cluster; -1 elsewhere. */
function assignClusters(changed: boolean[]): { ids: number[]; count: number } {
  const n = changed.length;
  const ids = new Array<number>(n).fill(-1);
  let count = 0;
  let i = 0;
  while (i < n) {
    if (!changed[i]) {
      i++;
      continue;
    }
    const start = i;
    let end = i;
    let j = i;
    while (j < n) {
      if (changed[j]) {
        end = j;
        j++;
        continue;
      }
      let k = j;
      while (k < n && !changed[k]) k++;
      if (k < n && k - j < CLUSTER_GAP) {
        j = k;
        continue;
      }
      break;
    }
    for (let r = start; r <= end; r++) ids[r] = count;
    count++;
    i = end + 1;
  }
  return { ids, count };
}

/** Nearest enclosing scope header: closest preceding line with less indentation than the fold's resume line. */
const scopeCache = new WeakMap<string[], Map<number, string>>();
function findScopeLabel(texts: string[], foldEnd: number): string {
  // Called from every fold row on every render (hover, virtual-scroll tick): memoize per file.
  let byEnd = scopeCache.get(texts);
  if (!byEnd) scopeCache.set(texts, (byEnd = new Map()));
  const hit = byEnd.get(foldEnd);
  if (hit !== undefined) return hit;
  const label = scanScopeLabel(texts, foldEnd);
  byEnd.set(foldEnd, label);
  return label;
}

function scanScopeLabel(texts: string[], foldEnd: number): string {
  const indentOf = (s: string) => {
    let w = 0;
    for (const ch of s) {
      if (ch === ' ') w++;
      else if (ch === '\t') w += 2;
      else break;
    }
    return w;
  };
  let base = -1;
  for (let k = foldEnd; k < Math.min(texts.length, foldEnd + 4); k++) {
    if (texts[k].trim()) {
      base = indentOf(texts[k]);
      break;
    }
  }
  if (base <= 0) return '';
  for (let k = foldEnd - 1; k >= 0; k--) {
    const t = texts[k];
    const trimmed = t.trim();
    if (!trimmed || /^[}\])>,;]+$/.test(trimmed)) continue;
    if (indentOf(t) < base) {
      const label = trimmed.replace(/\s*[{[(]\s*$/, '');
      return label.length > 64 ? `${label.slice(0, 63)}…` : label;
    }
  }
  return '';
}

function splitLinesPreserve(val: string): string[] {
  if (!val) return [];
  const lines = val.split(/\r?\n/);
  if (lines.length > 0 && lines[lines.length - 1] === '') {
    lines.pop();
  }
  return lines;
}

function renderSyntaxSpans(
  text: string,
  tokens: TokenSpan[] | undefined,
  changedRanges: CharRange[],
  wordBgVar: string
): React.ReactNode {
  if (!text) return '\u00A0';

  const baseTokens: TokenSpan[] = tokens && tokens.length > 0 ? tokens : [{ text }];

  if (changedRanges.length === 0) {
    return baseTokens.map((sp, i) => (
      <span
        key={i}
        style={{
          color: sp.color,
          fontStyle: sp.italic ? 'italic' : undefined,
          fontWeight: sp.bold ? 500 : undefined,
          textDecoration: sp.underline ? 'underline' : sp.strike ? 'line-through' : undefined,
        }}
      >
        {sp.text}
      </span>
    ));
  }

  const out: React.ReactNode[] = [];
  let cursor = 0;
  let rIdx = 0;
  let keyCounter = 0;

  for (const sp of baseTokens) {
    const spStart = cursor;
    const spEnd = cursor + sp.text.length;
    cursor = spEnd;

    let pos = spStart;
    while (rIdx < changedRanges.length && changedRanges[rIdx].to <= spStart) {
      rIdx++;
    }

    let j = rIdx;
    while (j < changedRanges.length && changedRanges[j].from < spEnd) {
      const r = changedRanges[j];
      const segStart = Math.max(pos, r.from);
      const segEnd = Math.min(spEnd, r.to);

      if (segStart > pos) {
        out.push(
          <span
            key={keyCounter++}
            style={{
              color: sp.color,
              fontStyle: sp.italic ? 'italic' : undefined,
              fontWeight: sp.bold ? 500 : undefined,
              textDecoration: sp.underline ? 'underline' : sp.strike ? 'line-through' : undefined,
            }}
          >
            {sp.text.slice(pos - spStart, segStart - spStart)}
          </span>
        );
      }

      if (segEnd > segStart) {
        out.push(
          <span
            key={keyCounter++}
            style={{
              color: sp.color,
              fontStyle: sp.italic ? 'italic' : undefined,
              fontWeight: sp.bold ? 500 : undefined,
              textDecoration: sp.underline ? 'underline' : sp.strike ? 'line-through' : undefined,
              background: wordBgVar,
              borderRadius: 3,
              padding: '0.5px 0',
            }}
          >
            {sp.text.slice(segStart - spStart, segEnd - spStart)}
          </span>
        );
        pos = segEnd;
      }

      if (r.to <= spEnd) {
        j++;
      } else {
        break;
      }
    }

    if (pos < spEnd) {
      out.push(
        <span
          key={keyCounter++}
          style={{
            color: sp.color,
            fontStyle: sp.italic ? 'italic' : undefined,
            fontWeight: sp.bold ? 500 : undefined,
            textDecoration: sp.underline ? 'underline' : sp.strike ? 'line-through' : undefined,
          }}
        >
          {sp.text.slice(pos - spStart)}
        </span>
      );
    }
  }

  return out;
}

function buildHunkPatch(
  filePath: string,
  oldStart: number,
  oldCount: number,
  newStart: number,
  newCount: number,
  lines: string[]
): string {
  const headerOldStart = oldCount === 0 ? Math.max(0, oldStart - 1) : oldStart;
  const headerNewStart = newCount === 0 ? Math.max(0, newStart - 1) : newStart;
  return [
    `--- a/${filePath}`,
    `+++ b/${filePath}`,
    `@@ -${headerOldStart},${oldCount} +${headerNewStart},${newCount} @@`,
    ...lines,
    '',
  ].join('\n');
}

function buildFoldedItems<T>(
  rows: T[],
  changed: boolean[],
  clusterIds: number[],
  compact: boolean,
  expandedRanges: Array<{ from: number; to: number }>
): FoldedItem<T>[] {
  const n = rows.length;

  const visible = new Array<boolean>(n).fill(!compact);
  if (compact) {
    for (let i = 0; i < n; i++) {
      if (changed[i]) {
        const lo = Math.max(0, i - CONTEXT_LINES);
        const hi = Math.min(n - 1, i + CONTEXT_LINES);
        for (let k = lo; k <= hi; k++) visible[k] = true;
      }
    }
    for (const r of expandedRanges) {
      for (let k = Math.max(0, r.from); k < Math.min(n, r.to); k++) visible[k] = true;
    }
  }

  const pushRow = (items: FoldedItem<T>[], k: number) => {
    const cluster = clusterIds[k];
    items.push({
      kind: 'row',
      row: rows[k],
      index: k,
      cluster,
      clusterStart: cluster >= 0 && (k === 0 || clusterIds[k - 1] !== cluster),
    });
  };

  const items: FoldedItem<T>[] = [];
  let i = 0;
  while (i < n) {
    if (visible[i]) {
      pushRow(items, i);
      i++;
    } else {
      const start = i;
      while (i < n && !visible[i]) i++;
      const count = i - start;
      if (count < MIN_FOLD_LINES) {
        for (let k = start; k < i; k++) pushRow(items, k);
      } else {
        items.push({ kind: 'fold', id: `fold-${start}-${i}`, startIndex: start, endIndex: i, count });
      }
    }
  }

  return items;
}


const ROW_ALT = 'opacity-0 group-hover:opacity-100 absolute right-1.5 top-0.5 px-1.5 py-0.5 text-[10px] font-sans rounded bg-[var(--bg-elevated)] border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--accent)] hover:text-[var(--accent-fg)] cursor-pointer transition-opacity';

interface RowCommon {
  cluster: number;
  clusterStart: boolean;
  isActive: boolean;
  live: boolean;
  liveClass: string;
  pill: React.ReactNode;
  wrapClass: string;
  emptyBg: string;
  onEnter: (cluster: number) => void;
  registerRef: (cluster: number, el: HTMLTableRowElement | null) => void;
  onAsk: (line: number | null, text: string) => void;
}

const SplitRowView = React.memo(function SplitRowView({
  row,
  leftSpans,
  rightSpans,
  cluster,
  clusterStart,
  isActive,
  live,
  liveClass,
  pill,
  wrapClass,
  emptyBg,
  onEnter,
  registerRef,
  onAsk,
}: RowCommon & { row: SplitRow; leftSpans?: TokenSpan[]; rightSpans?: TokenSpan[] }) {
  const leftRanges = useMemo(
    () => (row.pairHighlight ? computeChangedRanges(row.left.text, row.right.text, 'left') : []),
    [row]
  );
  const rightRanges = useMemo(
    () => (row.pairHighlight ? computeChangedRanges(row.left.text, row.right.text, 'right') : []),
    [row]
  );
  const leftBg =
    row.left.type === 'remove' ? 'var(--removed-bg)' : row.left.type === 'empty' ? emptyBg : 'transparent';
  const rightBg =
    row.right.type === 'add' ? 'var(--added-bg)' : row.right.type === 'empty' ? emptyBg : 'transparent';
  const gutter =
    'select-none text-right pr-2.5 pl-1.5 text-[11px] tabular-nums text-[var(--text-dim)] border-r border-[var(--border)]/70 align-top';

  return (
    <tr
      data-r="1"
      ref={(el) => {
        if (clusterStart) registerRef(cluster, el);
      }}
      data-live={live ? '1' : undefined}
      onMouseEnter={cluster >= 0 ? () => onEnter(cluster) : undefined}
      className={`group ${live ? liveClass : ''} ${
        isActive ? 'outline outline-1 outline-[var(--accent)]/60' : ''
      }`}
    >
      <td
        className={gutter}
        style={{
          background: leftBg,
          boxShadow: row.left.type === 'remove' ? 'inset 3px 0 0 var(--removed-fg)' : undefined,
        }}
      >
        {row.left.line ?? ''}
      </td>
      <td
        className={`px-2.5 border-r border-[var(--border)] align-top relative ${wrapClass}`}
        style={{ background: leftBg }}
      >
        {row.left.type === 'remove' && (
          <span className="select-none mr-1.5 font-semibold text-[var(--removed-fg)]">-</span>
        )}
        {row.left.type !== 'empty' &&
          renderSyntaxSpans(row.left.text, leftSpans, leftRanges, 'var(--removed-word)')}
        {row.left.type === 'remove' && (
          <button
            onClick={() => onAsk(row.left.line, row.left.text)}
            className={ROW_ALT}
            title="Quote this deleted line to Agent Chat"
          >
            Ask
          </button>
        )}
      </td>
      <td
        className={gutter}
        style={{
          background: rightBg,
          boxShadow: row.right.type === 'add' ? 'inset 3px 0 0 var(--added-fg)' : undefined,
        }}
      >
        {row.right.line ?? ''}
      </td>
      <td className={`px-2.5 align-top relative ${wrapClass}`} style={{ background: rightBg }}>
        {row.right.type === 'add' && (
          <span className="select-none mr-1.5 font-semibold text-[var(--added-fg)]">+</span>
        )}
        {row.right.type !== 'empty' &&
          renderSyntaxSpans(row.right.text, rightSpans, rightRanges, 'var(--added-word)')}
        {pill}
        {row.right.line !== null && !pill && (
          <button
            onClick={() => onAsk(row.right.line, row.right.text)}
            className={ROW_ALT}
            title="Quote this line to Agent Chat"
          >
            Ask
          </button>
        )}
      </td>
    </tr>
  );
});

const UnifiedRowView = React.memo(function UnifiedRowView({
  row,
  spans,
  cluster,
  clusterStart,
  isActive,
  live,
  liveClass,
  pill,
  wrapClass,
  onEnter,
  registerRef,
  onAsk,
}: Omit<RowCommon, 'emptyBg'> & { row: UnifiedRow; spans?: TokenSpan[] }) {
  const changedRanges = useMemo(
    () =>
      row.pairedText !== undefined
        ? row.type === 'remove'
          ? computeChangedRanges(row.text, row.pairedText, 'left')
          : computeChangedRanges(row.pairedText, row.text, 'right')
        : [],
    [row]
  );
  const wordBg = row.type === 'add' ? 'var(--added-word)' : 'var(--removed-word)';
  const accentStripe =
    row.type === 'add'
      ? 'inset 3px 0 0 var(--added-fg)'
      : row.type === 'remove'
      ? 'inset 3px 0 0 var(--removed-fg)'
      : undefined;

  return (
    <tr
      data-r="1"
      ref={(el) => {
        if (clusterStart) registerRef(cluster, el);
      }}
      data-live={live ? '1' : undefined}
      onMouseEnter={cluster >= 0 ? () => onEnter(cluster) : undefined}
      className={`group ${live ? liveClass : ''} ${
        isActive ? 'outline outline-1 outline-[var(--accent)]/60' : ''
      }`}
      style={{
        background:
          row.type === 'add' ? 'var(--added-bg)' : row.type === 'remove' ? 'var(--removed-bg)' : 'transparent',
      }}
    >
      <td
        className="w-12 select-none text-right pr-2.5 pl-1.5 text-[11px] tabular-nums text-[var(--text-dim)] border-r border-[var(--border)]/70 align-top"
        style={{ boxShadow: accentStripe }}
      >
        {row.oldLine ?? ''}
      </td>
      <td className="w-12 select-none text-right pr-2.5 pl-1.5 text-[11px] tabular-nums text-[var(--text-dim)] border-r border-[var(--border)]/70 align-top">
        {row.newLine ?? ''}
      </td>
      <td className="w-6 select-none text-center font-semibold align-top">
        {row.type === 'add' ? (
          <span className="text-[var(--added-fg)]">+</span>
        ) : row.type === 'remove' ? (
          <span className="text-[var(--removed-fg)]">-</span>
        ) : (
          ' '
        )}
      </td>
      <td className={`px-2.5 relative align-top ${wrapClass}`}>
        {renderSyntaxSpans(row.text, spans, changedRanges, wordBg)}
        {pill}
        {!pill && (
          <button onClick={() => onAsk(row.newLine ?? row.oldLine, row.text)} className={ROW_ALT}>
            Ask
          </button>
        )}
      </td>
    </tr>
  );
});

/** Windowing: only rows near the viewport are mounted once a diff is long. */
const VIRTUAL_MIN_ITEMS = 300;
/** Above this, wrapping is off (fixed-height rows keep scrolling and tab switches fast). */
const HUGE_ITEMS = 800;
const OVERSCAN_PX = 900;
const WINDOW_CHUNK = 24;

export const DiffViewer: React.FC<DiffViewerProps> = ({
  diff,
  fileStatus,
  commitContext,
  fileNav,
  theme,
  loading,
  onStage,
  onUnstage,
  onDiscard,
  onApplyHunk,
  onSwitchToCode,
  onAskAgent,
  onToast,
  liveFlash,
  followEdits,
  userActivityRef,
}) => {
  const [preferredViewMode, setPreferredViewMode] = useState<'split' | 'unified'>('split');
  // Side-by-side needs room: below this container width fall back to unified automatically
  // (container-based, not viewport-based, because panes are user-resizable).
  const [rootEl, setRootEl] = useState<HTMLDivElement | null>(null);
  const [narrow, setNarrow] = useState(false);
  const [rootW, setRootW] = useState(1200);
  useEffect(() => {
    if (!rootEl) return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0].contentRect.width;
      setRootW(Math.round(w / 8) * 8);
      setNarrow((prev) => (prev ? w < 600 : w < 560));
    });
    ro.observe(rootEl);
    return () => ro.disconnect();
  }, [rootEl]);
  const viewMode: 'split' | 'unified' = narrow ? 'unified' : preferredViewMode;
  const [compactMode, setCompactMode] = useState<boolean>(true);
  const [wordWrap, setWordWrap] = useState<boolean>(true);
  const [ignoreWhitespace, setIgnoreWhitespace] = useState<boolean>(false);
  const [expandedRanges, setExpandedRanges] = useState<Array<{ from: number; to: number }>>([]);
  const [activeHunk, setActiveHunk] = useState<number>(-1);
  const [copiedPath, setCopiedPath] = useState(false);
  const [hunkBusy, setHunkBusy] = useState<number | null>(null);
  const [hoverHunk, setHoverHunk] = useState<number>(-1);

  const hunkRefs = useRef<Record<number, HTMLTableRowElement | null>>({});
  const scrollContainerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setExpandedRanges([]);
    setActiveHunk(-1);
    hunkRefs.current = {};
    scrollContainerRef.current?.scrollTo(0, 0);
    setWin({ start: 0, end: WINDOW_CHUNK * 7 });
  }, [diff?.path, diff?.staged, commitContext?.hash]);

  const syntaxTokens = useMemo(() => {
    if (!diff || diff.binary) {
      return { oldLines: [] as TokenSpan[][], newLines: [] as TokenSpan[][] };
    }
    return {
      oldLines: highlightLines((diff.original || '').replace(/\r\n/g, '\n'), diff.path, theme),
      newLines: highlightLines((diff.modified || '').replace(/\r\n/g, '\n'), diff.path, theme),
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [diff?.original, diff?.modified, diff?.path, diff?.binary, theme]);

  const computed = useMemo(() => {
    if (!diff || diff.binary) {
      return {
        unified: [] as UnifiedRow[],
        split: [] as SplitRow[],
        unifiedChanged: [] as boolean[],
        splitChanged: [] as boolean[],
        hunks: [] as HunkMeta[],
        adds: 0,
        dels: 0,
        eolMismatch: false,
      };
    }

    // Index (LF) vs working tree (CRLF) would otherwise show every line as changed.
    const rawOld = diff.original || '';
    const rawNew = diff.modified || '';
    const eolMismatch = rawOld.includes('\r\n') !== rawNew.includes('\r\n');
    const changes = diffLines(
      eolMismatch ? rawOld.replace(/\r\n/g, '\n') : rawOld,
      eolMismatch ? rawNew.replace(/\r\n/g, '\n') : rawNew,
      { ignoreWhitespace }
    );
    const unified: UnifiedRow[] = [];
    const split: SplitRow[] = [];

    let oldNo = 1;
    let newNo = 1;
    let adds = 0;
    let dels = 0;

    let i = 0;
    while (i < changes.length) {
      const part = changes[i];
      if (!part.added && !part.removed) {
        const lines = splitLinesPreserve(part.value);
        for (const text of lines) {
          unified.push({ type: 'context', oldLine: oldNo, newLine: newNo, text, cluster: -1 });
          split.push({
            left: { line: oldNo, text, type: 'context' },
            right: { line: newNo, text, type: 'context' },
            cluster: -1,
          });
          oldNo++;
          newNo++;
        }
        i++;
      } else if (part.removed) {
        const remLines = splitLinesPreserve(part.value);
        const hasPair = i + 1 < changes.length && changes[i + 1].added;
        const addLines = hasPair ? splitLinesPreserve(changes[i + 1].value) : [];
        dels += remLines.length;
        adds += addLines.length;

        const startOld = oldNo;
        const startNew = newNo;
        const pairCount = Math.min(remLines.length, addLines.length);
        for (let k = 0; k < remLines.length; k++) {
          unified.push({
            type: 'remove',
            oldLine: oldNo++,
            newLine: null,
            text: remLines[k],
            pairedText: k < pairCount ? addLines[k] : undefined,
            cluster: -1,
          });
        }
        for (let k = 0; k < addLines.length; k++) {
          unified.push({
            type: 'add',
            oldLine: null,
            newLine: newNo++,
            text: addLines[k],
            pairedText: k < pairCount ? remLines[k] : undefined,
            cluster: -1,
          });
        }

        const maxLen = Math.max(remLines.length, addLines.length);
        for (let k = 0; k < maxLen; k++) {
          const hasL = k < remLines.length;
          const hasR = k < addLines.length;
          split.push({
            left: hasL
              ? { line: startOld + k, text: remLines[k], type: 'remove' }
              : { line: null, text: '', type: 'empty' },
            right: hasR
              ? { line: startNew + k, text: addLines[k], type: 'add' }
              : { line: null, text: '', type: 'empty' },
            pairHighlight: hasL && hasR,
            cluster: -1,
          });
        }
        i += hasPair ? 2 : 1;
      } else {
        const lines = splitLinesPreserve(part.value);
        adds += lines.length;
        const startNew = newNo;
        for (const text of lines) {
          unified.push({ type: 'add', oldLine: null, newLine: newNo++, text, cluster: -1 });
        }
        for (let k = 0; k < lines.length; k++) {
          split.push({
            left: { line: null, text: '', type: 'empty' },
            right: { line: startNew + k, text: lines[k], type: 'add' },
            cluster: -1,
          });
        }
        i++;
      }
    }

    const unifiedChanged = unified.map((r) => r.type !== 'context');
    const splitChanged = split.map((r) => r.left.type !== 'context' || r.right.type !== 'context');
    const uc = assignClusters(unifiedChanged);
    const sc = assignClusters(splitChanged);
    unified.forEach((r, idx) => (r.cluster = uc.ids[idx]));
    split.forEach((r, idx) => (r.cluster = sc.ids[idx]));

    // One patch per cluster, built from the unified rows (interior context included).
    const hunks: HunkMeta[] = [];
    let o = 1;
    let nw = 1;
    let cur: { start: number; oldStart: number; newStart: number } | null = null;
    const finish = (endRow: number, id: number) => {
      if (!cur) return;
      const lines: string[] = [];
      const excerpt: string[] = [];
      let oldCount = 0;
      let newCount = 0;
      for (let k = cur.start; k <= endRow; k++) {
        const r = unified[k];
        if (r.type === 'context') {
          lines.push(` ${r.text}`);
          oldCount++;
          newCount++;
        } else if (r.type === 'remove') {
          lines.push(`-${r.text}`);
          excerpt.push(`-${r.text}`);
          oldCount++;
        } else {
          lines.push(`+${r.text}`);
          excerpt.push(`+${r.text}`);
          newCount++;
        }
      }
      hunks.push({
        index: id,
        oldStart: cur.oldStart,
        oldCount,
        newStart: cur.newStart,
        newCount,
        patch: buildHunkPatch(diff.path, cur.oldStart, oldCount, cur.newStart, newCount, lines),
        firstRow: cur.start,
        lastRow: endRow,
        excerpt: excerpt.join('\n'),
      });
      cur = null;
    };
    for (let k = 0; k < unified.length; k++) {
      const r = unified[k];
      const id = r.cluster;
      if (id >= 0 && !cur) cur = { start: k, oldStart: o, newStart: nw };
      if (cur && id < 0) finish(k - 1, hunks.length);
      if (r.type === 'context') {
        o++;
        nw++;
      } else if (r.type === 'remove') o++;
      else nw++;
    }
    if (cur) finish(unified.length - 1, hunks.length);

    return { unified, split, unifiedChanged, splitChanged, hunks, adds, dels, eolMismatch };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [diff?.original, diff?.modified, diff?.path, diff?.binary, ignoreWhitespace]);

  // Only the active layout is folded/built; the other one costs nothing until it is selected.
  const splitItems = useMemo(
    () =>
      viewMode !== 'split'
        ? []
        : buildFoldedItems(
            computed.split,
            computed.splitChanged,
            computed.split.map((r) => r.cluster),
            compactMode,
            expandedRanges
          ),
    [viewMode, computed.split, computed.splitChanged, compactMode, expandedRanges]
  );

  const unifiedItems = useMemo(
    () =>
      viewMode !== 'unified'
        ? []
        : buildFoldedItems(
            computed.unified,
            computed.unifiedChanged,
            computed.unified.map((r) => r.cluster),
            compactMode,
            expandedRanges
          ),
    [viewMode, computed.unified, computed.unifiedChanged, compactMode, expandedRanges]
  );

  // Plain-text lines (new side where present) used to label folds with their enclosing scope.
  const splitScopeTexts = useMemo(
    () =>
      viewMode !== 'split'
        ? []
        : computed.split.map((r) => (r.right.type !== 'empty' ? r.right.text : r.left.text)),
    [viewMode, computed.split]
  );
  const unifiedScopeTexts = useMemo(
    () => (viewMode !== 'unified' ? [] : computed.unified.map((r) => r.text)),
    [viewMode, computed.unified]
  );

  const activeItems = viewMode === 'split' ? splitItems : unifiedItems;
  // Windowing needs uniform row heights: true whenever nothing wraps. If every line fits its
  // column, wrapping is moot, so long diffs are windowed even with wrap on.
  const longestLine = useMemo(() => {
    let m = 0;
    for (const r of computed.unified) if (r.text.length > m) m = r.text.length;
    return m + 2;
  }, [computed.unified]);
  const colPx =
    viewMode === 'split' ? (rootW - 10 - 2 * 48 - 2 * 20) / 2 : rootW - 10 - 96 - 24 - 20;
  const fitsOneLine = longestLine <= Math.floor(colPx / 8.2);
  const large = activeItems.length > VIRTUAL_MIN_ITEMS;
  const virtual = large && (fitsOneLine || !wordWrap || activeItems.length > HUGE_ITEMS);
  // Wrapping is switched off only when a long diff has long lines (wrapped rows can't be windowed).
  const huge = virtual && !fitsOneLine && wordWrap;
  const effectiveWrap = wordWrap && !huge;

  // Measured row / fold heights (rows are single-line when virtual, so heights are uniform).
  const [dims, setDims] = useState({ row: 20, fold: 25 });
  const offsets = useMemo(() => {
    const o = new Float64Array(activeItems.length + 1);
    for (let i = 0; i < activeItems.length; i++) {
      o[i + 1] = o[i] + (activeItems[i].kind === 'fold' ? dims.fold : dims.row);
    }
    return o;
  }, [activeItems, dims]);

  const [win, setWin] = useState({ start: 0, end: WINDOW_CHUNK * 7 });
  const recomputeWin = useCallback(() => {
    const el = scrollContainerRef.current;
    if (!el || !virtual) return;
    const n = activeItems.length;
    const find = (y: number) => {
      let lo = 0;
      let hi = n;
      while (lo < hi) {
        const m = (lo + hi) >> 1;
        if (offsets[m + 1] <= y) lo = m + 1;
        else hi = m;
      }
      return lo;
    };
    const top = el.scrollTop;
    const start = Math.max(0, Math.floor(find(top - OVERSCAN_PX) / WINDOW_CHUNK) * WINDOW_CHUNK);
    const end = Math.min(
      n,
      Math.ceil((find(top + el.clientHeight + OVERSCAN_PX) + 1) / WINDOW_CHUNK) * WINDOW_CHUNK
    );
    setWin((p) => (p.start === start && p.end === end ? p : { start, end }));
  }, [virtual, offsets, activeItems.length]);

  const scrollRafRef = useRef(0);
  const onScrollerScroll = useCallback(() => {
    if (scrollRafRef.current) return;
    scrollRafRef.current = requestAnimationFrame(() => {
      scrollRafRef.current = 0;
      recomputeWin();
    });
  }, [recomputeWin]);

  useLayoutEffect(() => {
    recomputeWin();
  }, [recomputeWin, diff?.path]);

  useEffect(() => {
    const el = scrollContainerRef.current;
    if (!el || !virtual) return;
    const ro = new ResizeObserver(() => recomputeWin());
    ro.observe(el);
    return () => ro.disconnect();
  }, [virtual, recomputeWin, diff?.path]);

  // Measure real row heights so spacer math never drifts (zoom, fonts, borders).
  useEffect(() => {
    if (!virtual) return;
    const sc = scrollContainerRef.current;
    if (!sc) return;
    const r = sc.querySelector('tr[data-r]');
    const f = sc.querySelector('tr[data-f]');
    setDims((p) => {
      const nr = r ? r.getBoundingClientRect().height : p.row;
      const nf = f ? f.getBoundingClientRect().height : p.fold;
      return Math.abs(nr - p.row) < 0.05 && Math.abs(nf - p.fold) < 0.05 ? p : { row: nr, fold: nf };
    });
  }, [virtual, win.start, win.end, activeItems, effectiveWrap]);

  /** Rows marked for the live edit: its added rows ('add'), or for a pure deletion the removed
   * rows at its anchor line ('del'; removed rows have no new-side line number to match), or just
   * the anchor row ('at': scroll target only, no glow). */
  const liveRows = useMemo(() => {
    const live = new Map<number, 'add' | 'del' | 'at'>();
    if (!liveFlash) return live;
    const newLine = (it: (typeof activeItems)[number]) => {
      if (it.kind !== 'row') return null;
      return viewMode === 'split' ? (it.row as SplitRow).right.line : (it.row as UnifiedRow).newLine;
    };
    const isAdd = (it: (typeof activeItems)[number]) =>
      it.kind === 'row' &&
      (viewMode === 'split' ? (it.row as SplitRow).right.type === 'add' : (it.row as UnifiedRow).type === 'add');
    const isRemoveOnly = (it: (typeof activeItems)[number]) =>
      it.kind === 'row' &&
      (viewMode === 'split'
        ? (it.row as SplitRow).left.type === 'remove' && (it.row as SplitRow).right.type === 'empty'
        : (it.row as UnifiedRow).type === 'remove');
    for (const it of activeItems) {
      if (it.kind !== 'row' || !isAdd(it)) continue;
      const l = newLine(it);
      if (l !== null && liveFlash.ranges.some((r) => l >= r.from && l <= r.to)) live.set(it.index, 'add');
    }
    if (live.size === 0 && liveFlash.anchor) {
      const at = activeItems.findIndex((it) => newLine(it) === liveFlash.anchor);
      if (at >= 0) {
        // Deleted rows sit just above the anchor line (or just below it at the end of the file).
        for (let i = at - 1; i >= 0 && isRemoveOnly(activeItems[i]); i--) {
          live.set((activeItems[i] as { index: number }).index, 'del');
        }
        if (live.size === 0) {
          for (let i = at + 1; i < activeItems.length && isRemoveOnly(activeItems[i]); i++) {
            live.set((activeItems[i] as { index: number }).index, 'del');
          }
        }
        if (live.size === 0) live.set((activeItems[at] as { index: number }).index, 'at');
      }
    }
    return live;
  }, [liveFlash, activeItems, viewMode]);

  const scrollToItem = useCallback(
    (idx: number, smooth: boolean) => {
      const el = scrollContainerRef.current;
      if (!el || idx < 0) return;
      el.scrollTo({
        top: Math.max(0, offsets[idx] - el.clientHeight / 2 + dims.row),
        behavior: smooth ? 'smooth' : 'auto',
      });
    },
    [offsets, dims.row]
  );

  // Live edits: scroll to the lines an agent just wrote (unless the user is busy scrolling/typing).
  const handledFlashRef = useRef(0);
  useEffect(() => {
    if (!liveFlash || handledFlashRef.current === liveFlash.key) return;
    if (!followEdits && !liveFlash.reveal) return;
    const idx = activeItems.findIndex((it) => it.kind === 'row' && liveRows.has(it.index));
    if (idx < 0) return;
    handledFlashRef.current = liveFlash.key;
    if (!liveFlash.reveal && userActivityRef && Date.now() - userActivityRef.current < 4000) return;
    if (virtual) scrollToItem(idx, true);
    else
      requestAnimationFrame(() =>
        scrollContainerRef.current
          ?.querySelector('[data-live="1"]')
          ?.scrollIntoView({ behavior: 'smooth', block: 'center' })
      );
  }, [liveFlash, followEdits, activeItems, liveRows, virtual, scrollToItem, userActivityRef]);

  const askLineRef = useRef<(line: number | null, text: string) => void>(() => {});
  const onAsk = useCallback((line: number | null, text: string) => askLineRef.current(line, text), []);
  const onEnterCluster = useCallback((c: number) => setHoverHunk(c), []);
  const registerRef = useCallback((c: number, el: HTMLTableRowElement | null) => {
    hunkRefs.current[c] = el;
  }, []);

  const totalHunks = computed.hunks.length;

  const jumpHunk = (dir: -1 | 1) => {
    if (totalHunks === 0) return;
    const next =
      activeHunk < 0
        ? dir === 1
          ? 0
          : totalHunks - 1
        : (activeHunk + dir + totalHunks) % totalHunks;
    setActiveHunk(next);
    if (virtual) {
      scrollToItem(
        activeItems.findIndex((it) => it.kind === 'row' && it.clusterStart && it.cluster === next),
        true
      );
    } else {
      hunkRefs.current[next]?.scrollIntoView({ behavior: 'smooth', block: 'center' });
    }
  };

  // VS Code F7 / Shift+F7 hunk navigation shortcut
  const jumpHunkRef = useRef(jumpHunk);
  jumpHunkRef.current = jumpHunk;
  const totalHunksRef = useRef(totalHunks);
  totalHunksRef.current = totalHunks;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'F7' && totalHunksRef.current > 0) {
        e.preventDefault();
        jumpHunkRef.current(e.shiftKey ? -1 : 1);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  // Overview Ruler markers (proportional vertical tick marks like VS Code's diff minimap)
  const overviewMarkers = useMemo(() => {
    const list = viewMode === 'split' ? splitItems : unifiedItems;
    const total = list.length;
    if (total === 0) return [] as Array<{ topPct: number; heightPct: number; kind: 'add' | 'del' | 'mod' }>;

    const markers: Array<{ topPct: number; heightPct: number; kind: 'add' | 'del' | 'mod' }> = [];
    let i = 0;
    while (i < total) {
      const it = list[i];
      let kind: 'add' | 'del' | 'mod' | null = null;
      if (it.kind === 'row') {
        if (viewMode === 'split') {
          const r = it.row as SplitRow;
          const hasDel = r.left.type === 'remove';
          const hasAdd = r.right.type === 'add';
          if (hasDel && hasAdd) kind = 'mod';
          else if (hasAdd) kind = 'add';
          else if (hasDel) kind = 'del';
        } else {
          const r = it.row as UnifiedRow;
          if (r.type === 'add') kind = 'add';
          else if (r.type === 'remove') kind = 'del';
        }
      }
      if (!kind) {
        i++;
        continue;
      }
      const start = i;
      while (i < total) {
        const cur = list[i];
        if (cur.kind !== 'row') break;
        let ck: 'add' | 'del' | 'mod' | null = null;
        if (viewMode === 'split') {
          const r = cur.row as SplitRow;
          const hasDel = r.left.type === 'remove';
          const hasAdd = r.right.type === 'add';
          if (hasDel && hasAdd) ck = 'mod';
          else if (hasAdd) ck = 'add';
          else if (hasDel) ck = 'del';
        } else {
          const r = cur.row as UnifiedRow;
          if (r.type === 'add') ck = 'add';
          else if (r.type === 'remove') ck = 'del';
        }
        if (ck !== kind) break;
        i++;
      }
      markers.push({
        topPct: (start / total) * 100,
        heightPct: Math.max(0.8, ((i - start) / total) * 100),
        kind,
      });
    }
    return markers;
  }, [viewMode, splitItems, unifiedItems]);

  const expandRange = (from: number, to: number) => {
    setExpandedRanges((prev) => [...prev, { from, to }]);
  };

  const canApplyHunks =
    !commitContext &&
    !ignoreWhitespace &&
    !computed.eolMismatch &&
    !!onApplyHunk &&
    !!fileStatus &&
    !fileStatus.untracked;

  const triggerHunkAction = async (
    hunk: HunkMeta,
    action: 'stage' | 'unstage' | 'revert'
  ) => {
    if (!onApplyHunk) return;
    setHunkBusy(hunk.index);
    try {
      await onApplyHunk(hunk.patch, action);
    } finally {
      setHunkBusy(null);
    }
  };

  if (loading && !diff) {
    return (
      <div className="flex-1 flex items-center justify-center text-xs text-[var(--text-muted)]">
        Loading diff...
      </div>
    );
  }

  if (!diff) {
    return (
      <EmptyState
        size="lg"
        icon={GitDiff}
        title="No diff selected"
        description="Pick a changed file or a commit to inspect its changes."
        className="flex-1"
      />
    );
  }

  const askAboutWholeDiff = () => {
    const excerpt = (diff.patch || diff.modified).slice(0, 3500);
    onAskAgent(`Review the changes in \`${diff.path}\`:\n\`\`\`diff\n${excerpt}\n\`\`\``);
  };

  const askAboutLine = (lineNum: number | null, text: string) => {
    onAskAgent(
      `Regarding \`${diff.path}${lineNum ? `:${lineNum}` : ''}\`:\n\`\`\`\n${text}\n\`\`\`\n`
    );
  };

  askLineRef.current = askAboutLine;

  const askAboutHunk = (hunk: HunkMeta) => {
    const first = computed.unified[hunk.firstRow];
    const lineNum = first ? first.newLine ?? first.oldLine : null;
    onAskAgent(
      `Regarding the change in \`${diff.path}${lineNum ? `:${lineNum}` : ''}\`:
\`\`\`diff
${hunk.excerpt.slice(0, 3500)}
\`\`\`
`
    );
  };

  const copyPath = async () => {
    try {
      await navigator.clipboard.writeText(diff.path);
      setCopiedPath(true);
      setTimeout(() => setCopiedPath(false), 1400);
      onToast?.(`Copied ${diff.path}`);
    } catch {
      // ignore
    }
  };

  // Flat recessed fill for the empty side of a split diff (no busy diagonal hatching).
  const emptyCellBg = theme.type === 'dark' ? 'rgba(0, 0, 0, 0.16)' : 'rgba(15, 23, 42, 0.035)';

  const contextBadge = commitContext
    ? commitContext.mode === 'commit'
      ? `${commitContext.shortHash}^ → ${commitContext.shortHash}`
      : `${commitContext.shortHash} → Working Tree`
    : diff.staged
    ? 'HEAD → Staged'
    : 'Index → Working Tree';

  const pathSlash = diff.path.lastIndexOf('/');
  const pathDir = pathSlash >= 0 ? diff.path.slice(0, pathSlash + 1) : '';
  const pathName = pathSlash >= 0 ? diff.path.slice(pathSlash + 1) : diff.path;

  const cellWrapClass = effectiveWrap ? 'whitespace-pre-wrap break-words' : 'whitespace-pre';

  const renderFoldRow = (
    item: { id: string; startIndex: number; endIndex: number; count: number },
    colSpan: number,
    scopeTexts: string[]
  ) => {
    const scope = findScopeLabel(scopeTexts, item.endIndex);
    return (
      <tr key={item.id} data-f="1" className="select-none">
        <td
          colSpan={colSpan}
          className="h-6 px-3 text-[11px] font-sans text-[var(--text-muted)] border-y border-dashed border-[var(--border)]/60 bg-[var(--bg-sidebar)]/50"
        >
          <div className="flex items-center justify-center gap-1.5 min-w-0">
            {item.count > EXPAND_STEP && (
              <button
                onClick={() => expandRange(item.endIndex - EXPAND_STEP, item.endIndex)}
                className="px-1.5 rounded hover:bg-[var(--bg-hover)] text-[var(--accent)] flex items-center gap-0.5 cursor-pointer"
                title={`Show ${EXPAND_STEP} lines above`}
              >
                <ChevronUp size={11.5} />+{EXPAND_STEP}
              </button>
            )}
            <button
              onClick={() => expandRange(item.startIndex, item.endIndex)}
              className="px-2 rounded hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] inline-flex items-center gap-1.5 min-w-0 cursor-pointer transition-colors"
              title={`Expand ${item.count} unchanged lines`}
            >
              <UnfoldVertical size={12} className="text-[var(--accent)] shrink-0" />
              <span className="shrink-0">{item.count} unchanged lines</span>
              {scope && (
                <span className="font-mono text-[10.5px] text-[var(--text-dim)] truncate">
                  · {scope}
                </span>
              )}
            </button>
            {item.count > EXPAND_STEP && (
              <button
                onClick={() => expandRange(item.startIndex, item.startIndex + EXPAND_STEP)}
                className="px-1.5 rounded hover:bg-[var(--bg-hover)] text-[var(--accent)] flex items-center gap-0.5 cursor-pointer"
                title={`Show ${EXPAND_STEP} lines below`}
              >
                <ChevronDown size={11.5} />+{EXPAND_STEP}
              </button>
            )}
          </div>
        </td>
      </tr>
    );
  };

  /** Floating pill pinned to the top-right of a change cluster (no dedicated row). */
  const renderHunkPill = (cluster: number) => {
    const hunk = computed.hunks[cluster];
    if (!hunk) return null;
    const busy = hunkBusy === hunk.index;
    const btn =
      'px-1.5 h-[22px] text-[10.5px] font-sans flex items-center gap-1 cursor-pointer disabled:opacity-40 hover:bg-[var(--bg-hover)] text-[var(--text-primary)]';
    return (
      <div
        className="absolute right-2 top-0.5 z-10 flex w-max items-stretch whitespace-nowrap rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] shadow-md overflow-hidden divide-x divide-[var(--border)]"
        onClick={(e) => e.stopPropagation()}
      >
        {canApplyHunks &&
          (diff.staged ? (
            <button
              disabled={busy}
              onClick={() => triggerHunkAction(hunk, 'unstage')}
              className={btn}
              title="Unstage this change"
            >
              <Minus size={10.5} />
              Unstage
            </button>
          ) : (
            <>
              <button
                disabled={busy}
                onClick={() => triggerHunkAction(hunk, 'stage')}
                className={btn}
                title="Stage this change"
              >
                <Plus size={10.5} />
                Stage
              </button>
              <button
                disabled={busy}
                onClick={() => triggerHunkAction(hunk, 'revert')}
                className={`${btn} text-[var(--removed-fg)]`}
                title="Revert this change"
              >
                <RotateCcw size={10.5} />
                Revert
              </button>
            </>
          ))}
        <button onClick={() => askAboutHunk(hunk)} className={btn} title="Quote this change to Agent Chat">
          <MessageSquarePlus size={10.5} />
          Ask
        </button>
      </div>
    );
  };

  const liveClass = liveFlash ? `live-flash-${liveFlash.key % 2}` : '';
  const liveDelClass = liveFlash ? `live-flash-del-${liveFlash.key % 2}` : '';
  /** Glow class per live row kind; the 'at' anchor row is only a scroll target. */
  const liveRowClass = (index: number) => {
    const k = liveRows.get(index);
    return k === 'add' ? liveClass : k === 'del' ? liveDelClass : '';
  };

  return (
    <div ref={setRootEl} className="@container flex-1 flex flex-col min-h-0 min-w-0 bg-[var(--bg-base)]">
      {/* Optional Commit / Compare Context Banner */}
      {commitContext && (
        <div className="flex items-center justify-between gap-2 px-3 py-1.5 border-b border-[var(--border)] bg-[var(--accent)]/[0.08] text-xs shrink-0">
          <div className="flex items-center gap-2 min-w-0">
            <GitCommitHorizontal size={14} className="text-[var(--accent)] shrink-0" />
            <span className="px-1.5 py-0.5 rounded bg-[var(--bg-elevated)] border border-[var(--border)] font-mono text-[11px] font-semibold text-[var(--accent)] shrink-0">
              {commitContext.shortHash}
            </span>
            <span className="font-medium text-[var(--text-primary)] truncate">
              {commitContext.subject}
            </span>
            {commitContext.author && (
              <span className="text-[11px] text-[var(--text-dim)] shrink-0 hidden @xl:inline">
                · {commitContext.author} {commitContext.date ? `(${commitContext.date})` : ''}
              </span>
            )}
          </div>
          <span className="text-[10.5px] uppercase tracking-wider font-semibold px-2 py-0.5 rounded bg-[var(--bg-elevated)] border border-[var(--border)] text-[var(--text-muted)] shrink-0">
            {commitContext.mode === 'commit' ? 'Commit Snapshot' : 'Compare vs Working Tree'}
          </span>
        </div>
      )}

      {/* Diff Toolbar */}
      <div className="flex items-center justify-between px-3 py-1.5 border-b border-[var(--border)] bg-[var(--bg-sidebar)] gap-2 shrink-0 flex-wrap">
        <div className="flex items-center gap-2 min-w-0">
          <span className="font-mono text-xs truncate" title={diff.path}>
            <span className="text-[var(--text-dim)]">{pathDir}</span>
            <span className="font-medium text-[var(--text-primary)]">{pathName}</span>
          </span>
          <button
            onClick={copyPath}
            className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-dim)] hover:text-[var(--text-primary)] cursor-pointer shrink-0"
            title="Copy relative file path"
          >
            {copiedPath ? <Check size={12} className="text-[var(--added-fg)]" /> : <Copy size={12} />}
          </button>
          <span
            className={`text-[10.5px] px-1.5 py-0.5 rounded border font-mono shrink-0 ${
              !commitContext && diff.staged
                ? 'bg-[var(--accent)] text-[var(--accent-fg)] border-transparent font-semibold'
                : 'bg-[var(--bg-elevated)] text-[var(--text-muted)] border-[var(--border)]'
            }`}
            title={commitContext ? commitContext.subject : 'Comparison shown'}
          >
            {contextBadge}
          </span>
          {computed.eolMismatch && (
            <span
              className="text-[10.5px] px-1.5 py-0.5 rounded border border-[var(--border)] text-[var(--text-muted)] shrink-0"
              title="Line endings differ between the two sides (CRLF vs LF); they are ignored here and hunk actions are disabled"
            >
              EOL ignored
            </span>
          )}
          <div className="flex items-center gap-1.5 shrink-0 tabular-nums">
            <span className="text-xs font-mono font-medium text-[var(--added-fg)]">
              +{computed.adds}
            </span>
            <span className="text-xs font-mono font-medium text-[var(--removed-fg)]">
              -{computed.dels}
            </span>
          </div>
        </div>

        <div className="flex items-center gap-1.5 flex-wrap min-w-0 ml-auto">
          {/* Prev / Next File navigation */}
          {fileNav && fileNav.total > 1 && (
            <div className="flex items-center rounded border border-[var(--border)] overflow-hidden bg-[var(--bg-elevated)]/40">
              <button
                onClick={fileNav.onPrev}
                className="px-1.5 py-1 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
                title="Previous changed file"
              >
                <ArrowLeft size={12.5} />
              </button>
              <span className="px-1.5 text-[11px] font-mono tabular-nums text-[var(--text-muted)] select-none border-x border-[var(--border)]">
                {fileNav.index + 1}/{fileNav.total}
              </span>
              <button
                onClick={fileNav.onNext}
                className="px-1.5 py-1 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
                title="Next changed file"
              >
                <ArrowRight size={12.5} />
              </button>
            </div>
          )}

          {/* Prev / Next Change Hunk Jump (F7 / Shift+F7) */}
          {totalHunks > 0 && (
            <div className="flex items-center rounded border border-[var(--border)] overflow-hidden bg-[var(--bg-elevated)]/40">
              <button
                onClick={() => jumpHunk(-1)}
                className="px-1.5 py-1 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
                title="Previous change (Shift+F7)"
              >
                <ChevronUp size={13} />
              </button>
              <span
                className="px-1.5 text-[11px] font-mono tabular-nums text-[var(--text-muted)] select-none border-x border-[var(--border)]"
                title="Change hunks"
              >
                {activeHunk >= 0 ? `${activeHunk + 1}/${totalHunks}` : `${totalHunks}`}
              </span>
              <button
                onClick={() => jumpHunk(1)}
                className="px-1.5 py-1 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
                title="Next change (F7)"
              >
                <ChevronDown size={13} />
              </button>
            </div>
          )}

          {/* Ignore Whitespace Toggle */}
          <button
            onClick={() => setIgnoreWhitespace((v) => !v)}
            className={`p-1.5 rounded border border-[var(--border)] cursor-pointer ${
              ignoreWhitespace
                ? 'bg-[var(--bg-active)] text-[var(--accent)]'
                : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
            }`}
            title={ignoreWhitespace ? 'Ignoring whitespace changes' : 'Ignore whitespace changes'}
          >
            <Space size={13} />
          </button>

          {/* Word Wrap Toggle */}
          <button
            onClick={() => setWordWrap((v) => !v)}
            className={`p-1.5 rounded border border-[var(--border)] cursor-pointer ${
              effectiveWrap
                ? 'bg-[var(--bg-active)] text-[var(--accent)]'
                : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
            }`}
            title={
              huge
                ? 'Wrap is paused for this large diff (keeps scrolling fast)'
                : wordWrap
                ? 'Word wrap ON (click to disable)'
                : 'Word wrap OFF (click to enable)'
            }
          >
            <WrapText size={13} />
          </button>

          {/* Compact vs Full Context Toggle */}
          <button
            onClick={() => {
              setCompactMode((v) => !v);
              setExpandedRanges([]);
            }}
            className={`px-2 py-1 text-xs rounded border border-[var(--border)] flex items-center gap-1 cursor-pointer ${
              compactMode
                ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
            }`}
            title={
              compactMode
                ? 'Showing compact diff (3 context lines). Click for full file.'
                : 'Showing full file. Click to fold unchanged sections.'
            }
          >
            {compactMode ? <FoldVertical size={13} /> : <UnfoldVertical size={13} />}
            <span className="hidden @3xl:inline">{compactMode ? 'Compact' : 'Full'}</span>
          </button>

          {/* Split vs Unified Toggle */}
          <div className="flex rounded border border-[var(--border)] overflow-hidden">
            <button
              onClick={() => setPreferredViewMode('split')}
              disabled={narrow}
              className={`px-2 py-1 text-xs flex items-center gap-1 cursor-pointer disabled:cursor-not-allowed ${
                viewMode === 'split'
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                  : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
              }`}
              title={narrow ? 'Side-by-side needs a wider pane' : 'Side-by-Side Diff'}
            >
              <Columns2 size={13} />
              <span className="hidden @2xl:inline">Split</span>
            </button>
            <button
              onClick={() => setPreferredViewMode('unified')}
              className={`px-2 py-1 text-xs flex items-center gap-1 cursor-pointer ${
                viewMode === 'unified'
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                  : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
              }`}
              title="Inline / Unified Diff"
            >
              <Rows3 size={13} />
              <span className="hidden @2xl:inline">Unified</span>
            </button>
          </div>

          <button
            onClick={askAboutWholeDiff}
            className="px-2 py-1 text-xs rounded border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
            title="Send this diff to Agent Chat"
          >
            <MessageSquarePlus size={13} />
            <span className="hidden @3xl:inline">Ask Agent</span>
          </button>

          {!commitContext && fileStatus && !fileStatus.staged && onStage && (
            <button
              onClick={onStage}
              className="px-2 py-1 text-xs rounded bg-[var(--accent)] text-[var(--accent-fg)] hover:opacity-90 flex items-center gap-1 cursor-pointer font-medium"
              title="Stage file"
            >
              <Plus size={13} />
              Stage
            </button>
          )}
          {!commitContext && fileStatus && fileStatus.staged && onUnstage && (
            <button
              onClick={onUnstage}
              className="px-2 py-1 text-xs rounded border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
              title="Unstage file"
            >
              <Minus size={13} />
              Unstage
            </button>
          )}
          {!commitContext && fileStatus && onDiscard && (
            <button
              onClick={onDiscard}
              className="p-1.5 text-xs rounded border border-[var(--border)] text-[var(--removed-fg)] hover:bg-[var(--removed-bg)] flex items-center cursor-pointer"
              title="Discard changes to this file"
            >
              <RotateCcw size={13} />
            </button>
          )}
          <button
            onClick={onSwitchToCode}
            className="px-2 py-1 text-xs rounded border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
            title={commitContext ? 'Open current working tree file in editor' : 'Open in editor'}
          >
            <Edit3 size={13} />
            <span className="hidden @2xl:inline">{commitContext ? 'Current' : 'Edit'}</span>
          </button>
        </div>
      </div>

      {/* Diff Body + VS Code Overview Ruler */}
      {diff.binary ? (
        <EmptyState
          size="lg"
          icon={Binary}
          title="Binary file changed"
          description="There's no text preview for this file type."
          className="flex-1"
        />
      ) : computed.adds === 0 && computed.dels === 0 ? (
        <EmptyState
          size="lg"
          icon={Equals}
          title={ignoreWhitespace ? 'Only whitespace changed' : 'No differences'}
          description={<span className="font-mono text-[11.5px]">{diff.path}</span>}
          action={<EmptyStateButton onClick={onSwitchToCode}>Open file</EmptyStateButton>}
          className="flex-1"
        />
      ) : (
        <div className="flex-1 flex min-h-0 relative overflow-hidden">
          <div
            ref={scrollContainerRef}
            onMouseLeave={() => setHoverHunk(-1)}
            onScroll={virtual ? onScrollerScroll : undefined}
            className="flex-1 overflow-auto font-mono text-[13px] leading-[20px]"
          >
            {(() => {
              const n = activeItems.length;
              const start = virtual ? Math.min(win.start, n) : 0;
              const end = virtual ? Math.min(win.end, n) : n;
              const topPad = virtual ? offsets[start] : 0;
              const botPad = virtual ? offsets[n] - offsets[end] : 0;
              const spacer = (h: number, key: string) => (
                <tr key={key} aria-hidden style={{ height: h }}>
                  <td colSpan={4} style={{ padding: 0, border: 0 }} />
                </tr>
              );
              const common = (cluster: number, clusterStart: boolean) => ({
                cluster,
                clusterStart,
                isActive: clusterStart && cluster === activeHunk,
                pill:
                  clusterStart && (hoverHunk === cluster || activeHunk === cluster)
                    ? renderHunkPill(cluster)
                    : null,
                liveClass,
                wrapClass: cellWrapClass,
                onEnter: onEnterCluster,
                registerRef,
                onAsk,
              });

              if (viewMode === 'split') {
                return (
                  <table
                    className={`w-full border-collapse ${effectiveWrap ? 'table-fixed' : 'min-w-full'}`}
                  >
                    <colgroup>
                      <col className="w-12" />
                      <col className="w-1/2" />
                      <col className="w-12" />
                      <col className="w-1/2" />
                    </colgroup>
                    <tbody>
                      {topPad > 0 && spacer(topPad, 'pad-top')}
                      {splitItems.slice(start, end).map((item) => {
                        if (item.kind === 'fold') return renderFoldRow(item, 4, splitScopeTexts);
                        const { row, index, cluster, clusterStart } = item;
                        return (
                          <SplitRowView
                            key={index}
                            row={row}
                            leftSpans={
                              row.left.line !== null ? syntaxTokens.oldLines[row.left.line - 1] : undefined
                            }
                            rightSpans={
                              row.right.line !== null ? syntaxTokens.newLines[row.right.line - 1] : undefined
                            }
                            live={liveRows.has(index)}
                            emptyBg={emptyCellBg}
                            {...common(cluster, clusterStart)}
                            liveClass={liveRowClass(index)}
                          />
                        );
                      })}
                      {botPad > 0 && spacer(botPad, 'pad-bottom')}
                    </tbody>
                  </table>
                );
              }

              return (
                <table className="w-full border-collapse">
                  <tbody>
                    {topPad > 0 && spacer(topPad, 'pad-top')}
                    {unifiedItems.slice(start, end).map((item) => {
                      if (item.kind === 'fold') return renderFoldRow(item, 4, unifiedScopeTexts);
                      const { row, index, cluster, clusterStart } = item;
                      const spans =
                        row.type === 'remove'
                          ? row.oldLine !== null
                            ? syntaxTokens.oldLines[row.oldLine - 1]
                            : undefined
                          : row.newLine !== null
                          ? syntaxTokens.newLines[row.newLine - 1]
                          : undefined;
                      return (
                        <UnifiedRowView
                          key={index}
                          row={row}
                          spans={spans}
                          live={liveRows.has(index)}
                          {...common(cluster, clusterStart)}
                          liveClass={liveRowClass(index)}
                        />
                      );
                    })}
                    {botPad > 0 && spacer(botPad, 'pad-bottom')}
                  </tbody>
                </table>
              );
            })()}
          </div>

          {/* VS Code Overview Ruler (clickable proportional diff minimap) */}
          <div
            onClick={(e) => {
              const scroller = scrollContainerRef.current;
              if (!scroller) return;
              const rect = e.currentTarget.getBoundingClientRect();
              const ratio = Math.max(0, Math.min(1, (e.clientY - rect.top) / rect.height));
              scroller.scrollTo({
                top: ratio * Math.max(0, scroller.scrollHeight - scroller.clientHeight),
                behavior: 'smooth',
              });
            }}
            className="w-[10px] shrink-0 relative bg-[var(--bg-sidebar)]/80 border-l border-[var(--border)]/60 cursor-pointer select-none"
            title="Diff overview — click to jump"
          >
            {overviewMarkers.map((m, idx) => (
              <div
                key={idx}
                style={{
                  top: `${m.topPct}%`,
                  height: `${m.heightPct}%`,
                  background:
                    m.kind === 'add'
                      ? 'var(--added-fg)'
                      : m.kind === 'del'
                      ? 'var(--removed-fg)'
                      : 'var(--modified-fg)',
                }}
                className="absolute left-[2px] right-[2px] rounded-[1px] opacity-80"
              />
            ))}
          </div>
        </div>
      )}
    </div>
  );
};
