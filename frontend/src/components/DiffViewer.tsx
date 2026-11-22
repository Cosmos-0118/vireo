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
}

interface UnifiedRow {
  type: 'context' | 'add' | 'remove';
  oldLine: number | null;
  newLine: number | null;
  text: string;
  pairedText?: string;
}

interface SplitRow {
  left: { line: number | null; text: string; type: 'context' | 'remove' | 'empty' };
  right: { line: number | null; text: string; type: 'context' | 'add' | 'empty' };
  pairHighlight?: boolean;
}

interface HunkMeta {
  index: number;
  oldStart: number;
  oldCount: number;
  newStart: number;
  newCount: number;
  patch: string;
}

type FoldedItem<T> =
  | { kind: 'hunk-header'; hunk: HunkMeta }
  | { kind: 'row'; row: T; index: number; hunkIndex?: number }
  | { kind: 'fold'; id: string; startIndex: number; endIndex: number; count: number };

const CONTEXT_LINES = 3;
const MIN_FOLD_LINES = 4;
const EXPAND_STEP = 12;

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
  isChanged: (row: T) => boolean,
  hunkByStartRow: Map<number, HunkMeta>,
  compact: boolean,
  expandedRanges: Array<{ from: number; to: number }>
): { items: FoldedItem<T>[]; hunkCount: number } {
  const n = rows.length;
  const changed = rows.map(isChanged);

  const hunkStarts = new Map<number, number>();
  let hunkCount = 0;
  for (let i = 0; i < n; i++) {
    if (changed[i] && (i === 0 || !changed[i - 1])) {
      hunkStarts.set(i, hunkCount++);
    }
  }

  const isRowVisible = (idx: number): boolean => {
    if (!compact) return true;
    for (const r of expandedRanges) {
      if (idx >= r.from && idx < r.to) return true;
    }
    return false;
  };

  const visible = new Array<boolean>(n).fill(!compact);
  if (compact) {
    for (let i = 0; i < n; i++) {
      if (changed[i]) {
        const lo = Math.max(0, i - CONTEXT_LINES);
        const hi = Math.min(n - 1, i + CONTEXT_LINES);
        for (let k = lo; k <= hi; k++) visible[k] = true;
      } else if (isRowVisible(i)) {
        visible[i] = true;
      }
    }
  }

  const items: FoldedItem<T>[] = [];
  let i = 0;
  while (i < n) {
    if (visible[i]) {
      const hunk = hunkByStartRow.get(i);
      if (hunk) {
        items.push({ kind: 'hunk-header', hunk });
      }
      items.push({
        kind: 'row',
        row: rows[i],
        index: i,
        hunkIndex: hunkStarts.get(i),
      });
      i++;
    } else {
      const start = i;
      while (i < n && !visible[i]) i++;
      const count = i - start;
      if (count < MIN_FOLD_LINES) {
        for (let k = start; k < i; k++) {
          const hunk = hunkByStartRow.get(k);
          if (hunk) items.push({ kind: 'hunk-header', hunk });
          items.push({
            kind: 'row',
            row: rows[k],
            index: k,
            hunkIndex: hunkStarts.get(k),
          });
        }
      } else {
        items.push({ kind: 'fold', id: `fold-${start}-${i}`, startIndex: start, endIndex: i, count });
      }
    }
  }

  return { items, hunkCount };
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
        unifiedHunkMap: new Map<number, HunkMeta>(),
        splitHunkMap: new Map<number, HunkMeta>(),
        adds: 0,
        dels: 0,
      };
    }

    const changes = diffLines(diff.original || '', diff.modified || '', {
      ignoreWhitespace,
    });
    const unified: UnifiedRow[] = [];
    const split: SplitRow[] = [];
    const unifiedHunkMap = new Map<number, HunkMeta>();
    const splitHunkMap = new Map<number, HunkMeta>();

    let oldNo = 1;
    let newNo = 1;
    let adds = 0;
    let dels = 0;
    let hunkIdx = 0;

    let i = 0;
    while (i < changes.length) {
      const part = changes[i];
      if (!part.added && !part.removed) {
        const lines = splitLinesPreserve(part.value);
        for (const text of lines) {
          unified.push({ type: 'context', oldLine: oldNo, newLine: newNo, text });
          split.push({
            left: { line: oldNo, text, type: 'context' },
            right: { line: newNo, text, type: 'context' },
          });
          oldNo++;
          newNo++;
        }
        i++;
      } else if (part.removed && i + 1 < changes.length && changes[i + 1].added) {
        const remLines = splitLinesPreserve(part.value);
        const addLines = splitLinesPreserve(changes[i + 1].value);
        dels += remLines.length;
        adds += addLines.length;

        const uStartRow = unified.length;
        const sStartRow = split.length;
        const startOld = oldNo;
        const startNew = newNo;

        const patch = buildHunkPatch(diff.path, startOld, remLines.length, startNew, addLines.length, [
          ...remLines.map((l) => `-${l}`),
          ...addLines.map((l) => `+${l}`),
        ]);
        const meta: HunkMeta = {
          index: hunkIdx++,
          oldStart: startOld,
          oldCount: remLines.length,
          newStart: startNew,
          newCount: addLines.length,
          patch,
        };
        unifiedHunkMap.set(uStartRow, meta);
        splitHunkMap.set(sStartRow, meta);

        const pairCount = Math.min(remLines.length, addLines.length);
        for (let k = 0; k < remLines.length; k++) {
          unified.push({
            type: 'remove',
            oldLine: oldNo++,
            newLine: null,
            text: remLines[k],
            pairedText: k < pairCount ? addLines[k] : undefined,
          });
        }
        for (let k = 0; k < addLines.length; k++) {
          unified.push({
            type: 'add',
            oldLine: null,
            newLine: newNo++,
            text: addLines[k],
            pairedText: k < pairCount ? remLines[k] : undefined,
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
          });
        }
        i += 2;
      } else if (part.removed) {
        const lines = splitLinesPreserve(part.value);
        dels += lines.length;
        const uStartRow = unified.length;
        const sStartRow = split.length;
        const startOld = oldNo;
        const startNew = newNo;

        const patch = buildHunkPatch(
          diff.path,
          startOld,
          lines.length,
          startNew,
          0,
          lines.map((l) => `-${l}`)
        );
        const meta: HunkMeta = {
          index: hunkIdx++,
          oldStart: startOld,
          oldCount: lines.length,
          newStart: startNew,
          newCount: 0,
          patch,
        };
        unifiedHunkMap.set(uStartRow, meta);
        splitHunkMap.set(sStartRow, meta);

        for (const text of lines) {
          unified.push({ type: 'remove', oldLine: oldNo, newLine: null, text });
          split.push({
            left: { line: oldNo, text, type: 'remove' },
            right: { line: null, text: '', type: 'empty' },
          });
          oldNo++;
        }
        i++;
      } else {
        const lines = splitLinesPreserve(part.value);
        adds += lines.length;
        const uStartRow = unified.length;
        const sStartRow = split.length;
        const startOld = oldNo;
        const startNew = newNo;

        const patch = buildHunkPatch(
          diff.path,
          startOld,
          0,
          startNew,
          lines.length,
          lines.map((l) => `+${l}`)
        );
        const meta: HunkMeta = {
          index: hunkIdx++,
          oldStart: startOld,
          oldCount: 0,
          newStart: startNew,
          newCount: lines.length,
          patch,
        };
        unifiedHunkMap.set(uStartRow, meta);
        splitHunkMap.set(sStartRow, meta);

        for (const text of lines) {
          unified.push({ type: 'add', oldLine: null, newLine: newNo, text });
          split.push({
            left: { line: null, text: '', type: 'empty' },
            right: { line: newNo, text, type: 'add' },
          });
          newNo++;
        }
        i++;
      }
    }

    return { unified, split, unifiedHunkMap, splitHunkMap, adds, dels };
  }, [diff, ignoreWhitespace]);

  const splitFolded = useMemo(
    () =>
      buildFoldedItems(
        computed.split,
        (r) => r.left.type !== 'context' || r.right.type !== 'context',
        computed.splitHunkMap,
        compactMode,
        expandedRanges
      ),
    [computed.split, computed.splitHunkMap, compactMode, expandedRanges]
  );

  const unifiedFolded = useMemo(
    () =>
      buildFoldedItems(
        computed.unified,
        (r) => r.type !== 'context',
        computed.unifiedHunkMap,
        compactMode,
        expandedRanges
      ),
    [computed.unified, computed.unifiedHunkMap, compactMode, expandedRanges]
  );

  const totalHunks =
    viewMode === 'split' ? splitFolded.hunkCount : unifiedFolded.hunkCount;

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
    const list = viewMode === 'split' ? splitFolded.items : unifiedFolded.items;
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
  }, [viewMode, splitFolded.items, unifiedFolded.items]);

  // Proportional 5-block mini diff stat bar
  const statBlocks = useMemo(() => {
    const total = computed.adds + computed.dels;
    if (total === 0) return [] as Array<'add' | 'del' | 'neutral'>;
    const blocks = 5;
    let addBlocks = Math.round((computed.adds / total) * blocks);
    let delBlocks = Math.round((computed.dels / total) * blocks);
    if (computed.adds > 0 && addBlocks === 0) {
      addBlocks = 1;
      if (delBlocks >= blocks) delBlocks = blocks - 1;
    }
    if (computed.dels > 0 && delBlocks === 0) {
      delBlocks = 1;
      if (addBlocks >= blocks) addBlocks = blocks - 1;
    }
    while (addBlocks + delBlocks > blocks) {
      if (addBlocks >= delBlocks) addBlocks--;
      else delBlocks--;
    }
    const neutralBlocks = Math.max(0, blocks - addBlocks - delBlocks);
    return [
      ...Array<'add'>(addBlocks).fill('add'),
      ...Array<'del'>(delBlocks).fill('del'),
      ...Array<'neutral'>(neutralBlocks).fill('neutral'),
    ];
  }, [computed.adds, computed.dels]);

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

  const emptyCellBg =
    theme.type === 'dark'
      ? 'repeating-linear-gradient(-45deg, transparent, transparent 5px, rgba(255,255,255,0.028) 5px, rgba(255,255,255,0.028) 6px)'
      : 'repeating-linear-gradient(-45deg, transparent, transparent 5px, rgba(15,23,42,0.04) 5px, rgba(15,23,42,0.04) 6px)';

  const leftPaneLabel = commitContext
    ? commitContext.mode === 'commit'
      ? `Parent (${commitContext.shortHash}^)`
      : `Commit (${commitContext.shortHash})`
    : diff.staged
    ? 'HEAD (Original)'
    : 'Index / HEAD (Original)';

  const rightPaneLabel = commitContext
    ? commitContext.mode === 'commit'
      ? `Commit (${commitContext.shortHash})`
      : 'Working Tree (Current)'
    : diff.staged
    ? 'Staged (Index)'
    : 'Working Tree (Modified)';

  const cellWrapClass = wordWrap ? 'whitespace-pre-wrap break-words' : 'whitespace-pre';

  const renderFoldRow = (
    item: { id: string; startIndex: number; endIndex: number; count: number },
    colSpan: number
  ) => (
    <tr
      key={item.id}
      className="bg-[var(--bg-sidebar)]/85 border-y border-[var(--border)]/70 select-none"
    >
      <td colSpan={colSpan} className="py-1 px-3 text-[11px] font-sans text-[var(--text-muted)]">
        <div className="flex items-center justify-center gap-2">
          {item.count > EXPAND_STEP && (
            <button
              onClick={() => expandRange(item.startIndex, item.startIndex + EXPAND_STEP)}
              className="px-1.5 py-0.5 rounded hover:bg-[var(--bg-hover)] text-[var(--accent)] flex items-center gap-0.5 cursor-pointer"
              title={`Expand ${EXPAND_STEP} lines below`}
            >
              <ChevronDown size={11.5} />
              +{EXPAND_STEP}
            </button>
          )}
          <button
            onClick={() => expandRange(item.startIndex, item.endIndex)}
            className="px-2 py-0.5 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] inline-flex items-center gap-1.5 cursor-pointer transition-colors"
          >
            <UnfoldVertical size={12} className="text-[var(--accent)]" />
            Expand {item.count} unchanged lines
          </button>
          {item.count > EXPAND_STEP && (
            <button
              onClick={() => expandRange(item.endIndex - EXPAND_STEP, item.endIndex)}
              className="px-1.5 py-0.5 rounded hover:bg-[var(--bg-hover)] text-[var(--accent)] flex items-center gap-0.5 cursor-pointer"
              title={`Expand ${EXPAND_STEP} lines above`}
            >
              <ChevronUp size={11.5} />
              +{EXPAND_STEP}
            </button>
          )}
        </div>
      </td>
    </tr>
  );

  const renderHunkHeaderRow = (hunk: HunkMeta, colSpan: number) => (
    <tr
      key={`hunk-${hunk.index}`}
      className="bg-[var(--accent)]/[0.07] border-y border-[var(--border)]/60 select-none"
    >
      <td colSpan={colSpan} className="px-3 py-[3px] font-mono text-[11px]">
        <div className="flex items-center justify-between gap-2">
          <span className="text-[var(--text-muted)]">
            @@ -{hunk.oldCount === 0 ? Math.max(0, hunk.oldStart - 1) : hunk.oldStart},
            {hunk.oldCount} +{hunk.newCount === 0 ? Math.max(0, hunk.newStart - 1) : hunk.newStart},
            {hunk.newCount} @@
          </span>
          {canApplyHunks && (
            <div className="flex items-center gap-1 font-sans">
              {diff.staged ? (
                <button
                  disabled={hunkBusy === hunk.index}
                  onClick={() => triggerHunkAction(hunk, 'unstage')}
                  className="px-2 py-0.5 text-[10.5px] rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] text-[var(--text-primary)] flex items-center gap-1 cursor-pointer disabled:opacity-40"
                  title="Unstage only this hunk"
                >
                  <Minus size={10.5} />
                  Unstage Hunk
                </button>
              ) : (
                <>
                  <button
                    disabled={hunkBusy === hunk.index}
                    onClick={() => triggerHunkAction(hunk, 'stage')}
                    className="px-2 py-0.5 text-[10.5px] rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--accent)] hover:text-[var(--accent-fg)] text-[var(--text-primary)] flex items-center gap-1 cursor-pointer disabled:opacity-40"
                    title="Stage only this hunk"
                  >
                    <Plus size={10.5} />
                    Stage Hunk
                  </button>
                  <button
                    disabled={hunkBusy === hunk.index}
                    onClick={() => triggerHunkAction(hunk, 'revert')}
                    className="px-2 py-0.5 text-[10.5px] rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--removed-bg)] text-[var(--removed-fg)] flex items-center gap-1 cursor-pointer disabled:opacity-40"
                    title="Revert only this hunk"
                  >
                    <RotateCcw size={10.5} />
                    Revert Hunk
                  </button>
                </>
              )}
            </div>
          )}
        </div>
      </td>
    </tr>
  );

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
          <span
            className="font-mono text-xs font-medium truncate text-[var(--text-primary)]"
            title={diff.path}
          >
            {diff.path}
          </span>
          <button
            onClick={copyPath}
            className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-dim)] hover:text-[var(--text-primary)] cursor-pointer shrink-0"
            title="Copy relative file path"
          >
            {copiedPath ? <Check size={12} className="text-[var(--added-fg)]" /> : <Copy size={12} />}
          </button>
          {!commitContext && diff.staged && (
            <span className="text-[10px] px-1.5 py-0.5 rounded bg-[var(--accent)] text-[var(--accent-fg)] font-semibold tracking-wide shrink-0">
              STAGED
            </span>
          )}
          <div className="flex items-center gap-1.5 shrink-0 tabular-nums">
            <span className="text-xs font-mono font-medium text-[var(--added-fg)]">
              +{computed.adds}
            </span>
            <span className="text-xs font-mono font-medium text-[var(--removed-fg)]">
              -{computed.dels}
            </span>
            {statBlocks.length > 0 && (
              <span
                className="inline-flex items-center gap-[2px] ml-0.5"
                title={`+${computed.adds} additions, -${computed.dels} deletions`}
              >
                {statBlocks.map((b, idx) => (
                  <span
                    key={idx}
                    className="w-1.5 h-1.5 rounded-[1.5px]"
                    style={{
                      background:
                        b === 'add'
                          ? 'var(--added-fg)'
                          : b === 'del'
                          ? 'var(--removed-fg)'
                          : 'var(--border)',
                    }}
                  />
                ))}
              </span>
            )}
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

      {/* Split Pane Sub-Headers (Original vs Modified) */}
      {!diff.binary && (computed.adds > 0 || computed.dels > 0) && viewMode === 'split' && (
        <div className="grid grid-cols-2 border-b border-[var(--border)] bg-[var(--bg-sidebar)]/55 text-[11px] font-medium text-[var(--text-muted)] shrink-0 select-none pr-[10px]">
          <div className="px-3 py-1 border-r border-[var(--border)] flex items-center justify-between">
            <span className="truncate">{leftPaneLabel}</span>
            {computed.dels > 0 && (
              <span className="font-mono text-[10.5px] text-[var(--removed-fg)]">
                -{computed.dels}
              </span>
            )}
          </div>
          <div className="px-3 py-1 flex items-center justify-between">
            <span className="truncate">{rightPaneLabel}</span>
            {computed.adds > 0 && (
              <span className="font-mono text-[10.5px] text-[var(--added-fg)]">
                +{computed.adds}
              </span>
            )}
          </div>
        </div>
      )}

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
                  {splitFolded.items.map((item) => {
                    if (item.kind === 'fold') {
                      return renderFoldRow(item, 4);
                    }
                    if (item.kind === 'hunk-header') {
                      return renderHunkHeaderRow(item.hunk, 4);
                    }

                    const { row, index, hunkIndex } = item;
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
                    const isActiveHunk = hunkIndex !== undefined && hunkIndex === activeHunk;

                    return (
                      <tr
                        key={index}
                        ref={(el) => {
                          if (hunkIndex !== undefined) {
                            hunkRefs.current[hunkIndex] = el;
                          }
                        }}
                        className={`group ${
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
                          className={`px-2.5 border-r border-[var(--border)] align-top ${cellWrapClass}`}
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
                          {row.right.line !== null && (
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
                  {unifiedFolded.items.map((item) => {
                    if (item.kind === 'fold') {
                      return renderFoldRow(item, 4);
                    }
                    if (item.kind === 'hunk-header') {
                      return renderHunkHeaderRow(item.hunk, 4);
                    }

                    const { row, index, hunkIndex } = item;
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

                    const isActiveHunk = hunkIndex !== undefined && hunkIndex === activeHunk;

                    return (
                      <tr
                        key={index}
                        ref={(el) => {
                          if (hunkIndex !== undefined) {
                            hunkRefs.current[hunkIndex] = el;
                          }
                        }}
                        className={`group ${
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
