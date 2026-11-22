import React, { useEffect, useMemo, useRef, useState } from 'react';
import { diffLines, diffWordsWithSpace } from 'diff';
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
function findScopeLabel(texts: string[], foldEnd: number): string {
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

interface CharRange {
  from: number;
  to: number;
}

function computeChangedRanges(
  oldText: string,
  newText: string,
  side: 'left' | 'right'
): CharRange[] {
  if (oldText.length > 1200 || newText.length > 1200) return [];
  const parts = diffWordsWithSpace(oldText, newText);
  const ranges: CharRange[] = [];
  let pos = 0;

  for (const p of parts) {
    if (side === 'left') {
      if (p.added) continue;
      const len = p.value.length;
      if (p.removed && len > 0) {
        ranges.push({ from: pos, to: pos + len });
      }
      pos += len;
    } else {
      if (p.removed) continue;
      const len = p.value.length;
      if (p.added && len > 0) {
        ranges.push({ from: pos, to: pos + len });
      }
      pos += len;
    }
  }
  return ranges;
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
  useEffect(() => {
    if (!rootEl) return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0].contentRect.width;
      setNarrow((prev) => (prev ? w < 600 : w < 560));
    });
    ro.observe(rootEl);
    return () => ro.disconnect();
  }, [rootEl]);
  const viewMode: 'split' | 'unified' = narrow ? 'unified' : preferredViewMode;
  const [compactMode, setCompactMode] = useState<boolean>(true);
  const [wordWrap, setWordWrap] = useState<boolean>(false);
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
  }, [diff?.path, diff?.staged, commitContext?.hash]);

  const syntaxTokens = useMemo(() => {
    if (!diff || diff.binary) {
      return { oldLines: [] as TokenSpan[][], newLines: [] as TokenSpan[][] };
    }
    return {
      oldLines: highlightLines(diff.original || '', diff.path, theme),
      newLines: highlightLines(diff.modified || '', diff.path, theme),
    };
  }, [diff, theme]);

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
      };
    }

    const changes = diffLines(diff.original || '', diff.modified || '', {
      ignoreWhitespace,
    });
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

    return { unified, split, unifiedChanged, splitChanged, hunks, adds, dels };
  }, [diff, ignoreWhitespace]);

  const splitItems = useMemo(
    () =>
      buildFoldedItems(
        computed.split,
        computed.splitChanged,
        computed.split.map((r) => r.cluster),
        compactMode,
        expandedRanges
      ),
    [computed.split, computed.splitChanged, compactMode, expandedRanges]
  );

  const unifiedItems = useMemo(
    () =>
      buildFoldedItems(
        computed.unified,
        computed.unifiedChanged,
        computed.unified.map((r) => r.cluster),
        compactMode,
        expandedRanges
      ),
    [computed.unified, computed.unifiedChanged, compactMode, expandedRanges]
  );

  // Plain-text lines (new side where present) used to label folds with their enclosing scope.
  const splitScopeTexts = useMemo(
    () => computed.split.map((r) => (r.right.type !== 'empty' ? r.right.text : r.left.text)),
    [computed.split]
  );
  const unifiedScopeTexts = useMemo(() => computed.unified.map((r) => r.text), [computed.unified]);

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
    const el = hunkRefs.current[next];
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    }
  };

  // Live edits: scroll to the lines an agent just wrote (unless the user is busy scrolling/typing).
  const handledFlashRef = useRef(0);
  useEffect(() => {
    if (!liveFlash || !followEdits || handledFlashRef.current === liveFlash.key) return;
    const el = scrollContainerRef.current?.querySelector('[data-live="1"]');
    if (!el) return;
    handledFlashRef.current = liveFlash.key;
    if (userActivityRef && Date.now() - userActivityRef.current < 4000) return;
    el.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }, [liveFlash, followEdits, diff, viewMode, userActivityRef]);

  // VS Code F7 / Shift+F7 hunk navigation shortcut
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'F7' && totalHunks > 0) {
        e.preventDefault();
        jumpHunk(e.shiftKey ? -1 : 1);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

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
      <div className="flex-1 flex items-center justify-center text-xs text-[var(--text-muted)]">
        Select a modified file or commit change to inspect its diff.
      </div>
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

  const cellWrapClass = wordWrap ? 'whitespace-pre-wrap break-words' : 'whitespace-pre';

  const renderFoldRow = (
    item: { id: string; startIndex: number; endIndex: number; count: number },
    colSpan: number,
    scopeTexts: string[]
  ) => {
    const scope = findScopeLabel(scopeTexts, item.endIndex);
    return (
      <tr key={item.id} className="select-none">
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
        className="absolute right-2 top-0.5 z-10 flex items-stretch rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] shadow-md overflow-hidden divide-x divide-[var(--border)]"
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

  const isLiveLine = (line: number | null): boolean =>
    !!liveFlash && line !== null && liveFlash.ranges.some((r) => line >= r.from && line <= r.to);
  const liveClass = liveFlash ? `live-flash-${liveFlash.key % 2}` : '';

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
              wordWrap
                ? 'bg-[var(--bg-active)] text-[var(--accent)]'
                : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
            }`}
            title={wordWrap ? 'Word wrap ON (click to disable)' : 'Word wrap OFF (click to enable)'}
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
        <div className="flex-1 flex items-center justify-center text-xs text-[var(--text-muted)]">
          Binary file changed. No text preview available.
        </div>
      ) : computed.adds === 0 && computed.dels === 0 ? (
        <div className="flex-1 flex flex-col items-center justify-center gap-2 text-xs text-[var(--text-muted)]">
          <Check size={24} className="text-[var(--added-fg)]" />
          <span>
            {ignoreWhitespace
              ? `No non-whitespace differences in ${diff.path}`
              : `No differences in ${diff.path}`}
          </span>
          <button
            onClick={onSwitchToCode}
            className="text-xs underline text-[var(--accent)] cursor-pointer"
          >
            Open file content
          </button>
        </div>
      ) : (
        <div className="flex-1 flex min-h-0 relative overflow-hidden">
          <div
            ref={scrollContainerRef}
            onMouseLeave={() => setHoverHunk(-1)}
            className="flex-1 overflow-auto font-mono text-[13px] leading-[1.6]"
          >
            {viewMode === 'split' ? (
              <table className={`w-full border-collapse ${wordWrap ? 'table-fixed' : 'min-w-full'}`}>
                <colgroup>
                  <col className="w-12" />
                  <col className="w-1/2" />
                  <col className="w-12" />
                  <col className="w-1/2" />
                </colgroup>
                <tbody>
                  {splitItems.map((item) => {
                    if (item.kind === 'fold') {
                      return renderFoldRow(item, 4, splitScopeTexts);
                    }

                    const { row, index, cluster, clusterStart } = item;
                    const leftRanges = row.pairHighlight
                      ? computeChangedRanges(row.left.text, row.right.text, 'left')
                      : [];
                    const rightRanges = row.pairHighlight
                      ? computeChangedRanges(row.left.text, row.right.text, 'right')
                      : [];
                    const leftSpans =
                      row.left.line !== null
                        ? syntaxTokens.oldLines[row.left.line - 1]
                        : undefined;
                    const rightSpans =
                      row.right.line !== null
                        ? syntaxTokens.newLines[row.right.line - 1]
                        : undefined;
                    const isActiveHunk = clusterStart && cluster === activeHunk;
                    const showPill = clusterStart && (hoverHunk === cluster || activeHunk === cluster);
                    const live = isLiveLine(row.right.line) && row.right.type === 'add';

                    return (
                      <tr
                        key={index}
                        ref={(el) => {
                          if (clusterStart) hunkRefs.current[cluster] = el;
                        }}
                        data-live={live ? '1' : undefined}
                        onMouseEnter={cluster >= 0 ? () => setHoverHunk(cluster) : undefined}
                        className={`group ${live ? liveClass : ''} ${
                          isActiveHunk ? 'outline outline-1 outline-[var(--accent)]/60' : ''
                        }`}
                      >
                        {/* Left gutter */}
                        <td
                          className="select-none text-right pr-2.5 pl-1.5 text-[11px] tabular-nums text-[var(--text-dim)] border-r border-[var(--border)]/70 align-top"
                          style={{
                            background:
                              row.left.type === 'remove'
                                ? 'var(--removed-bg)'
                                : row.left.type === 'empty'
                                ? emptyCellBg
                                : 'transparent',
                            boxShadow:
                              row.left.type === 'remove'
                                ? 'inset 3px 0 0 var(--removed-fg)'
                                : undefined,
                          }}
                        >
                          {row.left.line ?? ''}
                        </td>

                        {/* Left content */}
                        <td
                          className={`px-2.5 border-r border-[var(--border)] align-top relative ${cellWrapClass}`}
                          style={{
                            background:
                              row.left.type === 'remove'
                                ? 'var(--removed-bg)'
                                : row.left.type === 'empty'
                                ? emptyCellBg
                                : 'transparent',
                          }}
                        >
                          {row.left.type === 'remove' && (
                            <span className="select-none mr-1.5 font-semibold text-[var(--removed-fg)]">
                              -
                            </span>
                          )}
                          {row.left.type !== 'empty' &&
                            renderSyntaxSpans(
                              row.left.text,
                              leftSpans,
                              leftRanges,
                              'var(--removed-word)'
                            )}
                          {row.left.type === 'remove' && (
                            <button
                              onClick={() => askAboutLine(row.left.line, row.left.text)}
                              className="opacity-0 group-hover:opacity-100 absolute right-1.5 top-0.5 px-1.5 py-0.5 text-[10px] font-sans rounded bg-[var(--bg-elevated)] border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--accent)] hover:text-[var(--accent-fg)] cursor-pointer transition-opacity"
                              title="Quote this deleted line to Agent Chat"
                            >
                              Ask
                            </button>
                          )}
                        </td>

                        {/* Right gutter */}
                        <td
                          className="select-none text-right pr-2.5 pl-1.5 text-[11px] tabular-nums text-[var(--text-dim)] border-r border-[var(--border)]/70 align-top"
                          style={{
                            background:
                              row.right.type === 'add'
                                ? 'var(--added-bg)'
                                : row.right.type === 'empty'
                                ? emptyCellBg
                                : 'transparent',
                            boxShadow:
                              row.right.type === 'add'
                                ? 'inset 3px 0 0 var(--added-fg)'
                                : undefined,
                          }}
                        >
                          {row.right.line ?? ''}
                        </td>

                        {/* Right content */}
                        <td
                          className={`px-2.5 align-top relative ${cellWrapClass}`}
                          style={{
                            background:
                              row.right.type === 'add'
                                ? 'var(--added-bg)'
                                : row.right.type === 'empty'
                                ? emptyCellBg
                                : 'transparent',
                          }}
                        >
                          {row.right.type === 'add' && (
                            <span className="select-none mr-1.5 font-semibold text-[var(--added-fg)]">
                              +
                            </span>
                          )}
                          {row.right.type !== 'empty' &&
                            renderSyntaxSpans(
                              row.right.text,
                              rightSpans,
                              rightRanges,
                              'var(--added-word)'
                            )}
                          {showPill && renderHunkPill(cluster)}
                          {row.right.line !== null && !showPill && (
                            <button
                              onClick={() => askAboutLine(row.right.line, row.right.text)}
                              className="opacity-0 group-hover:opacity-100 absolute right-1.5 top-0.5 px-1.5 py-0.5 text-[10px] font-sans rounded bg-[var(--bg-elevated)] border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--accent)] hover:text-[var(--accent-fg)] cursor-pointer transition-opacity"
                              title="Quote this line to Agent Chat"
                            >
                              Ask
                            </button>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            ) : (
              <table className="w-full border-collapse">
                <tbody>
                  {unifiedItems.map((item) => {
                    if (item.kind === 'fold') {
                      return renderFoldRow(item, 4, unifiedScopeTexts);
                    }

                    const { row, index, cluster, clusterStart } = item;
                    const spans =
                      row.type === 'remove'
                        ? row.oldLine !== null
                          ? syntaxTokens.oldLines[row.oldLine - 1]
                          : undefined
                        : row.newLine !== null
                        ? syntaxTokens.newLines[row.newLine - 1]
                        : undefined;

                    const changedRanges =
                      row.pairedText !== undefined
                        ? row.type === 'remove'
                          ? computeChangedRanges(row.text, row.pairedText, 'left')
                          : computeChangedRanges(row.pairedText, row.text, 'right')
                        : [];

                    const wordBg =
                      row.type === 'add' ? 'var(--added-word)' : 'var(--removed-word)';

                    const accentStripe =
                      row.type === 'add'
                        ? 'inset 3px 0 0 var(--added-fg)'
                        : row.type === 'remove'
                        ? 'inset 3px 0 0 var(--removed-fg)'
                        : undefined;

                    const isActiveHunk = clusterStart && cluster === activeHunk;
                    const showPill = clusterStart && (hoverHunk === cluster || activeHunk === cluster);
                    const live = isLiveLine(row.newLine) && row.type === 'add';

                    return (
                      <tr
                        key={index}
                        ref={(el) => {
                          if (clusterStart) hunkRefs.current[cluster] = el;
                        }}
                        data-live={live ? '1' : undefined}
                        onMouseEnter={cluster >= 0 ? () => setHoverHunk(cluster) : undefined}
                        className={`group ${live ? liveClass : ''} ${
                          isActiveHunk ? 'outline outline-1 outline-[var(--accent)]/60' : ''
                        }`}
                        style={{
                          background:
                            row.type === 'add'
                              ? 'var(--added-bg)'
                              : row.type === 'remove'
                              ? 'var(--removed-bg)'
                              : 'transparent',
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
                        <td className={`px-2.5 relative align-top ${cellWrapClass}`}>
                          {renderSyntaxSpans(row.text, spans, changedRanges, wordBg)}
                          {showPill && renderHunkPill(cluster)}
                          <button
                            onClick={() => askAboutLine(row.newLine ?? row.oldLine, row.text)}
                            className="opacity-0 group-hover:opacity-100 absolute right-2 top-0.5 px-1.5 py-0.5 text-[10px] font-sans rounded bg-[var(--bg-elevated)] border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--accent)] hover:text-[var(--accent-fg)] cursor-pointer transition-opacity"
                          >
                            Ask
                          </button>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}
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
