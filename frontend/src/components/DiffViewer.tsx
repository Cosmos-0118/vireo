import React, { useMemo, useState } from 'react';
import { diffLines, diffWordsWithSpace } from 'diff';
import {
  Check,
  Columns2,
  Edit3,
  MessageSquarePlus,
  Minus,
  Plus,
  RotateCcw,
  Rows3,
} from 'lucide-react';
import type { GitDiffRes, GitFileStatus } from '../types';

interface DiffViewerProps {
  diff: GitDiffRes | null;
  fileStatus?: GitFileStatus;
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
}

interface SplitRow {
  left: { line: number | null; text: string; type: 'context' | 'remove' | 'empty' };
  right: { line: number | null; text: string; type: 'context' | 'add' | 'empty' };
  pairHighlight?: boolean;
}

function splitLinesPreserve(val: string): string[] {
  if (!val) return [];
  const lines = val.split(/\r?\n/);
  if (lines.length > 0 && lines[lines.length - 1] === '') {
    lines.pop();
  }
  return lines;
}

function renderWordDiff(oldText: string, newText: string, side: 'left' | 'right') {
  const parts = diffWordsWithSpace(oldText, newText);
  return parts.map((p, idx) => {
    if (side === 'left') {
      if (p.added) return null;
      return (
        <span
          key={idx}
          style={
            p.removed
              ? { background: 'var(--removed-word)', borderRadius: 2 }
              : undefined
          }
        >
          {p.value}
        </span>
      );
    } else {
      if (p.removed) return null;
      return (
        <span
          key={idx}
          style={
            p.added
              ? { background: 'var(--added-word)', borderRadius: 2 }
              : undefined
          }
        >
          {p.value}
        </span>
      );
    }
  });
}

export const DiffViewer: React.FC<DiffViewerProps> = ({
  diff,
  fileStatus,
  loading,
  onStage,
  onUnstage,
  onDiscard,
  onSwitchToCode,
  onAskAgent,
}) => {
  const [viewMode, setViewMode] = useState<'split' | 'unified'>('split');

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

        for (const text of remLines) {
          unified.push({ type: 'remove', oldLine: oldNo++, newLine: null, text });
        }
        for (const text of addLines) {
          unified.push({ type: 'add', oldLine: null, newLine: newNo++, text });
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

  return (
    <div className="flex-1 flex flex-col min-h-0 bg-[var(--bg-base)]">
      {/* Diff toolbar */}
      <div className="flex items-center justify-between px-3 py-1.5 border-b border-[var(--border)] bg-[var(--bg-sidebar)] gap-2 shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          <span className="font-mono text-xs font-medium truncate text-[var(--text-primary)]">
            {diff.path}
          </span>
          {diff.staged && (
            <span className="text-[10px] px-1.5 py-0.5 rounded bg-[var(--accent)] text-[var(--accent-fg)] font-medium">
              STAGED
            </span>
          )}
          <span className="text-xs font-mono text-[var(--added-fg)]">+{computed.adds}</span>
          <span className="text-xs font-mono text-[var(--removed-fg)]">-{computed.dels}</span>
        </div>

        <div className="flex items-center gap-1.5 shrink-0">
          <div className="flex rounded border border-[var(--border)] overflow-hidden">
            <button
              onClick={() => setViewMode('split')}
              className={`px-2 py-1 text-xs flex items-center gap-1 cursor-pointer ${
                viewMode === 'split'
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
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
                  ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
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
              className="px-2 py-1 text-xs rounded bg-[var(--accent)] text-[var(--accent-fg)] hover:opacity-90 flex items-center gap-1 cursor-pointer"
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
        <div className="flex-1 overflow-auto font-mono text-[12.5px] leading-[1.55]">
          <table className="w-full border-collapse table-fixed">
            <colgroup>
              <col className="w-11" />
              <col className="w-1/2" />
              <col className="w-11" />
              <col className="w-1/2" />
            </colgroup>
            <tbody>
              {computed.split.map((row, idx) => (
                <tr key={idx} className="group hover:brightness-110">
                  {/* Left gutter */}
                  <td
                    className="select-none text-right pr-2 text-[11px] text-[var(--text-dim)] border-r border-[var(--border)] align-top"
                    style={{
                      background:
                        row.left.type === 'remove'
                          ? 'var(--removed-bg)'
                          : 'transparent',
                    }}
                  >
                    {row.left.line ?? ''}
                  </td>
                  {/* Left content */}
                  <td
                    className="px-2 whitespace-pre-wrap break-all border-r border-[var(--border)] align-top"
                    style={{
                      background:
                        row.left.type === 'remove'
                          ? 'var(--removed-bg)'
                          : row.left.type === 'empty'
                          ? 'rgba(128,128,128,0.04)'
                          : 'transparent',
                    }}
                  >
                    {row.left.type === 'remove' && (
                      <span className="select-none mr-1 text-[var(--removed-fg)]">-</span>
                    )}
                    {row.pairHighlight
                      ? renderWordDiff(row.left.text, row.right.text, 'left')
                      : row.left.text}
                  </td>

                  {/* Right gutter */}
                  <td
                    className="select-none text-right pr-2 text-[11px] text-[var(--text-dim)] border-r border-[var(--border)] align-top relative"
                    style={{
                      background:
                        row.right.type === 'add' ? 'var(--added-bg)' : 'transparent',
                    }}
                  >
                    {row.right.line ?? ''}
                  </td>
                  {/* Right content */}
                  <td
                    className="px-2 whitespace-pre-wrap break-all align-top relative"
                    style={{
                      background:
                        row.right.type === 'add'
                          ? 'var(--added-bg)'
                          : row.right.type === 'empty'
                          ? 'rgba(128,128,128,0.04)'
                          : 'transparent',
                    }}
                  >
                    {row.right.type === 'add' && (
                      <span className="select-none mr-1 text-[var(--added-fg)]">+</span>
                    )}
                    {row.pairHighlight
                      ? renderWordDiff(row.left.text, row.right.text, 'right')
                      : row.right.text}
                    {row.right.line !== null && (
                      <button
                        onClick={() => askAboutLine(row.right.line, row.right.text)}
                        className="opacity-0 group-hover:opacity-100 absolute right-1 top-0.5 px-1.5 py-0.5 text-[10px] rounded bg-[var(--bg-elevated)] border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--accent)] hover:text-[var(--accent-fg)] cursor-pointer"
                        title="Quote this line to Agent Chat"
                      >
                        Ask
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="flex-1 overflow-auto font-mono text-[12.5px] leading-[1.55]">
          <table className="w-full border-collapse">
            <tbody>
              {computed.unified.map((row, idx) => (
                <tr
                  key={idx}
                  className="group"
                  style={{
                    background:
                      row.type === 'add'
                        ? 'var(--added-bg)'
                        : row.type === 'remove'
                        ? 'var(--removed-bg)'
                        : 'transparent',
                  }}
                >
                  <td className="w-11 select-none text-right pr-2 text-[11px] text-[var(--text-dim)] border-r border-[var(--border)]">
                    {row.oldLine ?? ''}
                  </td>
                  <td className="w-11 select-none text-right pr-2 text-[11px] text-[var(--text-dim)] border-r border-[var(--border)]">
                    {row.newLine ?? ''}
                  </td>
                  <td className="w-5 select-none text-center font-bold">
                    {row.type === 'add' ? (
                      <span className="text-[var(--added-fg)]">+</span>
                    ) : row.type === 'remove' ? (
                      <span className="text-[var(--removed-fg)]">-</span>
                    ) : (
                      ' '
                    )}
                  </td>
                  <td className="px-2 whitespace-pre-wrap break-all relative">
                    {row.text}
                    <button
                      onClick={() => askAboutLine(row.newLine ?? row.oldLine, row.text)}
                      className="opacity-0 group-hover:opacity-100 absolute right-2 top-0.5 px-1.5 py-0.5 text-[10px] rounded bg-[var(--bg-elevated)] border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--accent)] hover:text-[var(--accent-fg)] cursor-pointer"
                    >
                      Ask
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
};
