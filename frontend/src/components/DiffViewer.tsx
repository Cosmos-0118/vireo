import React, { useEffect, useMemo, useRef, useState } from 'react';
import { diffLines, diffWordsWithSpace } from 'diff';
import {
  Check,
  ChevronDown,
  ChevronUp,
  Columns2,
  Edit3,
  FoldVertical,
  MessageSquarePlus,
  Minus,
  Plus,
  RotateCcw,
  Rows3,
  UnfoldVertical,
} from 'lucide-react';
import { highlightLines, type TokenSpan } from '../syntax';
import type { LiteTheme } from '../themes';
import type { GitDiffRes, GitFileStatus } from '../types';

interface DiffViewerProps {
  diff: GitDiffRes | null;
  fileStatus?: GitFileStatus;
  theme: LiteTheme;
  loading: boolean;
  onStage?: () => void;
  onUnstage?: () => void;
  onDiscard?: () => void;
  onSwitchToCode: () => void;
  onAskAgent: (snippet: string) => void;
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

type FoldedItem<T> =
  | { kind: 'row'; row: T; index: number; hunkIndex?: number }
  | { kind: 'fold'; id: string; startIndex: number; count: number };

const CONTEXT_LINES = 3;
const MIN_FOLD_LINES = 4;

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
  // Skip expensive word diff on very long minified lines
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

  const baseTokens: TokenSpan[] =
    tokens && tokens.length > 0 ? tokens : [{ text }];

  if (changedRanges.length === 0) {
    return baseTokens.map((sp, i) => (
      <span
        key={i}
        style={{
          color: sp.color,
          fontStyle: sp.italic ? 'italic' : undefined,
          fontWeight: sp.bold ? 500 : undefined,
        }}
      >
        {sp.text}
      </span>
    ));
  }

  // Intersect token spans with word-change ranges so syntax colors stay intact
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
          }}
        >
          {sp.text.slice(pos - spStart)}
        </span>
      );
    }
  }

  return out;
}

function buildFoldedItems<T>(
  rows: T[],
  isChanged: (row: T) => boolean,
  compact: boolean,
  expandedFolds: Record<string, boolean>
): { items: FoldedItem<T>[]; hunkCount: number } {
  const n = rows.length;
  const changed = rows.map(isChanged);

  // Assign hunkIndex to the first row of each contiguous changed block
  const hunkStarts = new Map<number, number>();
  let hunkCount = 0;
  for (let i = 0; i < n; i++) {
    if (changed[i] && (i === 0 || !changed[i - 1])) {
      hunkStarts.set(i, hunkCount++);
    }
  }

  if (!compact) {
    return {
      items: rows.map((row, index) => ({
        kind: 'row',
        row,
        index,
        hunkIndex: hunkStarts.get(index),
      })),
      hunkCount,
    };
  }

  // Mark rows within CONTEXT_LINES of any changed row as visible
  const visible = new Array<boolean>(n).fill(false);
  for (let i = 0; i < n; i++) {
    if (changed[i]) {
      const lo = Math.max(0, i - CONTEXT_LINES);
      const hi = Math.min(n - 1, i + CONTEXT_LINES);
      for (let k = lo; k <= hi; k++) visible[k] = true;
    }
  }

  const items: FoldedItem<T>[] = [];
  let i = 0;
  while (i < n) {
    if (visible[i]) {
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
      const id = `fold-${start}-${i}`;
      if (count < MIN_FOLD_LINES || expandedFolds[id]) {
        for (let k = start; k < i; k++) {
          items.push({
            kind: 'row',
            row: rows[k],
            index: k,
            hunkIndex: hunkStarts.get(k),
          });
        }
      } else {
        items.push({ kind: 'fold', id, startIndex: start, count });
      }
    }
  }

  return { items, hunkCount };
}

export const DiffViewer: React.FC<DiffViewerProps> = ({
  diff,
  fileStatus,
  theme,
  loading,
  onStage,
  onUnstage,
  onDiscard,
  onSwitchToCode,
  onAskAgent,
}) => {
  const [viewMode, setViewMode] = useState<'split' | 'unified'>('split');
  const [compactMode, setCompactMode] = useState<boolean>(true);
  const [expandedFolds, setExpandedFolds] = useState<Record<string, boolean>>({});
  const [activeHunk, setActiveHunk] = useState<number>(-1);
  const hunkRefs = useRef<Record<number, HTMLTableRowElement | null>>({});

  useEffect(() => {
    setExpandedFolds({});
    setActiveHunk(-1);
    hunkRefs.current = {};
  }, [diff?.path, diff?.staged]);

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
      return { unified: [] as UnifiedRow[], split: [] as SplitRow[], adds: 0, dels: 0 };
    }
    const changes = diffLines(diff.original || '', diff.modified || '');
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

        const startOld = oldNo - remLines.length;
        const startNew = newNo - addLines.length;
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

    return { unified, split, adds, dels };
  }, [diff]);

  const splitFolded = useMemo(
    () =>
      buildFoldedItems(
        computed.split,
        (r) => r.left.type !== 'context' || r.right.type !== 'context',
        compactMode,
        expandedFolds
      ),
    [computed.split, compactMode, expandedFolds]
  );

  const unifiedFolded = useMemo(
    () =>
      buildFoldedItems(
        computed.unified,
        (r) => r.type !== 'context',
        compactMode,
        expandedFolds
      ),
    [computed.unified, compactMode, expandedFolds]
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

  if (loading && !diff) {
    return (
      <div className="flex-1 flex items-center justify-center text-[var(--text-muted)]">
        Loading diff...
      </div>
    );
  }

  if (!diff) {
    return (
      <div className="flex-1 flex items-center justify-center text-[var(--text-muted)]">
        Select a modified file to inspect its live diff.
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

  const emptyCellBg =
    theme.type === 'dark'
      ? 'repeating-linear-gradient(-45deg, transparent, transparent 5px, rgba(255,255,255,0.025) 5px, rgba(255,255,255,0.025) 6px)'
      : 'repeating-linear-gradient(-45deg, transparent, transparent 5px, rgba(15,23,42,0.04) 5px, rgba(15,23,42,0.04) 6px)';

  return (
    <div className="flex-1 flex flex-col min-h-0 bg-[var(--bg-base)]">
      {/* Diff toolbar */}
      <div className="flex items-center justify-between px-3 py-1.5 border-b border-[var(--border)] bg-[var(--bg-sidebar)] gap-2 shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          <span className="font-mono text-xs font-medium truncate text-[var(--text-primary)]">
            {diff.path}
          </span>
          {diff.staged && (
            <span className="text-[10px] px-1.5 py-0.5 rounded bg-[var(--accent)] text-[var(--accent-fg)] font-semibold tracking-wide">
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

        <div className="flex items-center gap-1.5 shrink-0">
          {/* Prev / Next Change Jump */}
          {totalHunks > 0 && (
            <div className="flex items-center rounded border border-[var(--border)] overflow-hidden bg-[var(--bg-elevated)]/40">
              <button
                onClick={() => jumpHunk(-1)}
                className="px-1.5 py-1 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
                title="Previous change"
              >
                <ChevronUp size={13} />
              </button>
              <span className="px-1.5 text-[11px] font-mono tabular-nums text-[var(--text-muted)] select-none border-x border-[var(--border)]">
                {activeHunk >= 0 ? `${activeHunk + 1}/${totalHunks}` : `${totalHunks}`}
              </span>
              <button
                onClick={() => jumpHunk(1)}
                className="px-1.5 py-1 text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] cursor-pointer"
                title="Next change"
              >
                <ChevronDown size={13} />
              </button>
            </div>
          )}

          {/* Compact vs Full Context Toggle */}
          <button
            onClick={() => {
              setCompactMode((v) => !v);
              setExpandedFolds({});
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
            {compactMode ? 'Compact' : 'Full'}
          </button>

          {/* Split vs Unified Toggle */}
          <div className="flex rounded border border-[var(--border)] overflow-hidden">
            <button
              onClick={() => setViewMode('split')}
              className={`px-2 py-1 text-xs flex items-center gap-1 cursor-pointer ${
                viewMode === 'split'
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                  : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
              }`}
              title="Side-by-Side Diff"
            >
              <Columns2 size={13} />
              Split
            </button>
            <button
              onClick={() => setViewMode('unified')}
              className={`px-2 py-1 text-xs flex items-center gap-1 cursor-pointer ${
                viewMode === 'unified'
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)] font-medium'
                  : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
              }`}
              title="Unified Diff"
            >
              <Rows3 size={13} />
              Unified
            </button>
          </div>

          <button
            onClick={askAboutWholeDiff}
            className="px-2 py-1 text-xs rounded border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
            title="Send this diff to Agent Chat"
          >
            <MessageSquarePlus size={13} />
            Ask Agent
          </button>

          {fileStatus && !fileStatus.staged && onStage && (
            <button
              onClick={onStage}
              className="px-2 py-1 text-xs rounded bg-[var(--accent)] text-[var(--accent-fg)] hover:opacity-90 flex items-center gap-1 cursor-pointer font-medium"
              title="Stage file"
            >
              <Plus size={13} />
              Stage
            </button>
          )}
          {fileStatus && fileStatus.staged && onUnstage && (
            <button
              onClick={onUnstage}
              className="px-2 py-1 text-xs rounded border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
              title="Unstage file"
            >
              <Minus size={13} />
              Unstage
            </button>
          )}
          {fileStatus && onDiscard && (
            <button
              onClick={onDiscard}
              className="px-2 py-1 text-xs rounded border border-[var(--border)] text-[var(--removed-fg)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
              title="Discard changes to this file"
            >
              <RotateCcw size={13} />
            </button>
          )}
          <button
            onClick={onSwitchToCode}
            className="px-2 py-1 text-xs rounded border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
            title="Open in quick editor"
          >
            <Edit3 size={13} />
            Edit
          </button>
        </div>
      </div>

      {/* Diff body */}
      {diff.binary ? (
        <div className="flex-1 flex items-center justify-center text-[var(--text-muted)]">
          Binary file changed — diff preview unavailable.
        </div>
      ) : computed.adds === 0 && computed.dels === 0 ? (
        <div className="flex-1 flex flex-col items-center justify-center gap-2 text-[var(--text-muted)]">
          <Check size={24} className="text-[var(--added-fg)]" />
          <span>No differences in {diff.path}</span>
          <button
            onClick={onSwitchToCode}
            className="text-xs underline text-[var(--accent)] cursor-pointer"
          >
            Open file content
          </button>
        </div>
      ) : viewMode === 'split' ? (
        <div className="flex-1 overflow-auto font-mono text-[13.5px] leading-[1.65]">
          <table className="w-full border-collapse table-fixed">
            <colgroup>
              <col className="w-12" />
              <col className="w-1/2" />
              <col className="w-12" />
              <col className="w-1/2" />
            </colgroup>
            <tbody>
              {splitFolded.items.map((item) => {
                if (item.kind === 'fold') {
                  return (
                    <tr
                      key={item.id}
                      onClick={() =>
                        setExpandedFolds((prev) => ({ ...prev, [item.id]: true }))
                      }
                      className="cursor-pointer bg-[var(--bg-sidebar)] hover:bg-[var(--bg-hover)] border-y border-[var(--border)]/70 transition-colors"
                    >
                      <td
                        colSpan={4}
                        className="py-1 px-3 text-center text-[11px] font-sans text-[var(--text-muted)] select-none"
                      >
                        <span className="inline-flex items-center gap-1.5">
                          <UnfoldVertical size={12} className="text-[var(--accent)]" />
                          Expand {item.count} unchanged lines
                        </span>
                      </td>
                    </tr>
                  );
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
                      className="px-2.5 whitespace-pre-wrap break-all border-r border-[var(--border)] align-top"
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
                      className="px-2.5 whitespace-pre-wrap break-all align-top relative"
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
        </div>
      ) : (
        <div className="flex-1 overflow-auto font-mono text-[13.5px] leading-[1.65]">
          <table className="w-full border-collapse">
            <tbody>
              {unifiedFolded.items.map((item) => {
                if (item.kind === 'fold') {
                  return (
                    <tr
                      key={item.id}
                      onClick={() =>
                        setExpandedFolds((prev) => ({ ...prev, [item.id]: true }))
                      }
                      className="cursor-pointer bg-[var(--bg-sidebar)] hover:bg-[var(--bg-hover)] border-y border-[var(--border)]/70 transition-colors"
                    >
                      <td
                        colSpan={4}
                        className="py-1 px-3 text-center text-[11px] font-sans text-[var(--text-muted)] select-none"
                      >
                        <span className="inline-flex items-center gap-1.5">
                          <UnfoldVertical size={12} className="text-[var(--accent)]" />
                          Expand {item.count} unchanged lines
                        </span>
                      </td>
                    </tr>
                  );
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
                    <td className="px-2.5 whitespace-pre-wrap break-all relative align-top">
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
        </div>
      )}
    </div>
  );
};
