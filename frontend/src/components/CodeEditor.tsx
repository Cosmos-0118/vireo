import React, { useEffect, useRef, useState } from 'react';
import { Annotation, EditorState, Extension, StateEffect, StateField } from '@codemirror/state';
import {
  Decoration,
  DecorationSet,
  EditorView,
  keymap,
  lineNumbers,
  highlightActiveLine,
  highlightActiveLineGutter,
} from '@codemirror/view';
import { diffLines } from 'diff';
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { syntaxHighlighting } from '@codemirror/language';
import { Code2, Eye, GitCompare, MessageSquarePlus, Save } from 'lucide-react';
import { buildHighlightStyle, getLanguageSupport, isMarkdownPath, semanticHighlighting } from '../syntax';
import { MarkdownMessage } from './MarkdownView';
import type { LiteTheme } from '../themes';

interface CodeEditorProps {
  path: string;
  content: string;
  binary?: boolean;
  theme: LiteTheme;
  hasDiff?: boolean;
  jumpToLine?: number;
  /** 1-based column / end column (UTF-16) to select on the jump line. */
  jumpToColumn?: number;
  jumpToEndColumn?: number;
  /** Changes on every navigation so repeat jumps to the same line re-run. */
  jumpKey?: number;
  onSave: (newContent: string) => Promise<void>;
  onSwitchToDiff?: () => void;
  onAskAgent: (snippet: string) => void;
  /** Scroll to lines changed on disk by an agent / external tool (paused while the user is busy). */
  followEdits?: boolean;
  /** Timestamp (ms) of the user's last scroll / key / pointer activity. */
  userActivityRef?: React.MutableRefObject<number>;
  onDirtyChange?: (dirty: boolean) => void;
}

/** Marks transactions that apply on-disk content (so they don't count as user edits). */
const externalChange = Annotation.define<boolean>();
const setFlash = StateEffect.define<number[]>();
const flashField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(deco, tr) {
    deco = deco.map(tr.changes);
    for (const e of tr.effects) {
      if (e.is(setFlash)) {
        deco = Decoration.set(
          e.value.map((pos) => Decoration.line({ class: 'cm-agent-flash' }).range(pos)),
          true
        );
      }
    }
    return deco;
  },
  provide: (f) => EditorView.decorations.from(f),
});

const normalizeNewlines = (t: string) => t.replace(/\r\n?/g, '\n');

/** Line-level edit script from old to new, plus the added ranges in new-doc offsets. */
function computeEdit(oldText: string, newText: string) {
  const changes: Array<{ from: number; to: number; insert: string }> = [];
  const added: Array<{ from: number; to: number }> = [];
  let oldPos = 0;
  let newPos = 0;
  for (const part of diffLines(oldText, newText)) {
    const len = part.value.length;
    if (part.added) {
      changes.push({ from: oldPos, to: oldPos, insert: part.value });
      added.push({ from: newPos, to: newPos + len });
      newPos += len;
    } else if (part.removed) {
      changes.push({ from: oldPos, to: oldPos + len, insert: '' });
      oldPos += len;
    } else {
      oldPos += len;
      newPos += len;
    }
  }
  return { changes, added };
}

/** Patch the live document in place: keeps scroll, selection and undo history intact. */
function applyExternalContent(view: EditorView, next: string, scroll: boolean): boolean {
  const { changes, added } = computeEdit(view.state.doc.toString(), next);
  if (changes.length === 0) return false;
  const newDoc = view.state.update({ changes }).newDoc;
  const starts = new Set<number>();
  for (const a of added) {
    const last = Math.max(a.from, a.to - 1);
    const endLine = newDoc.lineAt(Math.min(last, newDoc.length));
    let l = newDoc.lineAt(Math.min(a.from, newDoc.length));
    for (;;) {
      starts.add(l.from);
      if (l.to >= endLine.to || starts.size > 500) break;
      l = newDoc.line(l.number + 1);
    }
  }
  const first = added.length ? Math.min(added[0].from, newDoc.length) : undefined;
  view.dispatch({
    changes,
    annotations: externalChange.of(true),
    effects: [
      setFlash.of([...starts].sort((x, y) => x - y)),
      ...(scroll && first !== undefined
        ? [EditorView.scrollIntoView(first, { y: 'center' as const })]
        : []),
    ],
  });
  return true;
}

function jumpTo(view: EditorView, line: number, col?: number, colEnd?: number) {
  if (line < 1 || line > view.state.doc.lines) return;
  const info = view.state.doc.line(line);
  const anchor = col && col > 0 ? Math.min(info.from + col - 1, info.to) : info.from;
  const head = col && colEnd && colEnd >= col ? Math.min(info.from + colEnd - 1, info.to) : anchor;
  view.dispatch({
    selection: { anchor, head },
    effects: EditorView.scrollIntoView(anchor, { y: 'center' }),
  });
}

function langExtension(path: string, theme: LiteTheme): Extension[] {
  const support = getLanguageSupport(path);
  return support ? [support, ...semanticHighlighting(path, theme)] : [];
}

export const CodeEditor: React.FC<CodeEditorProps> = ({
  path,
  content,
  binary,
  theme,
  hasDiff,
  jumpToLine,
  jumpToColumn,
  jumpToEndColumn,
  jumpKey,
  onSave,
  onSwitchToDiff,
  onAskAgent,
  followEdits,
  userActivityRef,
  onDirtyChange,
}) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const [dirty, setDirty] = useState(false);
  const dirtyRef = useRef(false);
  const [saving, setSaving] = useState(false);
  /** On-disk content that arrived while the user had unsaved edits. */
  const [pendingExternal, setPendingExternal] = useState<string | null>(null);
  const [liveAt, setLiveAt] = useState(0);
  const carryRef = useRef<{ path: string; doc: string; anchor: number } | null>(null);
  const contentRef = useRef(content);
  contentRef.current = content;

  useEffect(() => {
    dirtyRef.current = dirty;
    onDirtyChange?.(dirty);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dirty]);
  useEffect(
    () => () => onDirtyChange?.(false),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    []
  );
  useEffect(() => {
    if (!liveAt) return;
    const t = setTimeout(() => setLiveAt(0), 3500);
    return () => clearTimeout(t);
  }, [liveAt]);
  const isMarkdown = isMarkdownPath(path);
  const [mode, setMode] = useState<'source' | 'preview'>(isMarkdown && !jumpToLine ? 'preview' : 'source');
  const [previewText, setPreviewText] = useState(content);

  // Markdown opens rendered; everything else (and jump-to-line) opens as source.
  useEffect(() => {
    setMode(isMarkdown && !jumpToLine ? 'preview' : 'source');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path]);

  useEffect(() => {
    setPreviewText(content);
  }, [content]);

  const switchMode = (next: 'source' | 'preview') => {
    if (next === 'preview' && viewRef.current) setPreviewText(viewRef.current.state.doc.toString());
    setMode(next);
    if (next === 'source') requestAnimationFrame(() => viewRef.current?.requestMeasure());
  };

  const handleSave = async () => {
    if (!viewRef.current) return;
    setSaving(true);
    try {
      await onSave(viewRef.current.state.doc.toString());
      setDirty(false);
    } finally {
      setSaving(false);
    }
  };

  useEffect(() => {
    if (!containerRef.current || binary) return;

    const saveRef = () => {
      handleSave();
      return true;
    };

    // A theme change recreates the view; carry unsaved edits across instead of reloading from disk.
    const carried = carryRef.current && carryRef.current.path === path ? carryRef.current : null;
    carryRef.current = null;

    const state = EditorState.create({
      doc: carried ? carried.doc : normalizeNewlines(contentRef.current),
      selection: carried ? { anchor: Math.min(carried.anchor, carried.doc.length) } : undefined,
      extensions: [
        flashField,
        lineNumbers(),
        highlightActiveLine(),
        highlightActiveLineGutter(),
        history(),
        keymap.of([
          { key: 'Mod-s', run: saveRef },
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        ...langExtension(path, theme),
        syntaxHighlighting(buildHighlightStyle(theme)),
        EditorView.updateListener.of((u) => {
          if (u.docChanged && !u.transactions.some((t) => t.annotation(externalChange))) {
            setDirty(true);
          }
        }),
      ],
    });

    const view = new EditorView({
      state,
      parent: containerRef.current,
    });
    viewRef.current = view;
    if (!carried) setDirty(false);
    setPendingExternal(null);

    if (jumpToLine) jumpTo(view, jumpToLine, jumpToColumn, jumpToEndColumn);

    return () => {
      carryRef.current = {
        path,
        doc: view.state.doc.toString(),
        anchor: view.state.selection.main.anchor,
      };
      view.destroy();
      viewRef.current = null;
    };
    // Recreate only for a different file or theme; content changes are patched in place below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, theme, binary]);

  // Apply content that changed on disk without destroying the editor (keeps scroll, cursor, undo).
  useEffect(() => {
    const view = viewRef.current;
    if (!view || binary) return;
    const next = normalizeNewlines(content);
    const cur = view.state.doc.toString();
    if (cur === next) {
      setPendingExternal(null);
      return;
    }
    if (dirtyRef.current) {
      setPendingExternal(next);
      return;
    }
    if (cur === '') {
      // First load of a file: replace silently, no flash.
      view.dispatch({
        changes: { from: 0, to: 0, insert: next },
        annotations: externalChange.of(true),
      });
      if (jumpToLine) jumpTo(view, jumpToLine, jumpToColumn, jumpToEndColumn);
      return;
    }
    const busy = !!userActivityRef && Date.now() - userActivityRef.current < 4000;
    if (applyExternalContent(view, next, !!followEdits && !busy)) setLiveAt(Date.now());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [content, binary]);

  const reloadFromDisk = () => {
    const view = viewRef.current;
    if (!view || pendingExternal === null) return;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: pendingExternal },
      annotations: externalChange.of(true),
    });
    setPendingExternal(null);
    setDirty(false);
  };

  useEffect(() => {
    const view = viewRef.current;
    if (!view || !jumpToLine) return;
    jumpTo(view, jumpToLine, jumpToColumn, jumpToEndColumn);
    view.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [jumpToLine, jumpToColumn, jumpToEndColumn, jumpKey]);

  if (binary) {
    return (
      <div className="flex-1 flex items-center justify-center p-6 text-center text-[var(--text-muted)]">
        Binary file cannot be displayed in the text editor.
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col min-h-0">
      <div className="flex items-center justify-between h-9 px-3.5 border-b border-[var(--border)] bg-[var(--bg-sidebar)] shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          <span className="font-mono text-[13px] truncate text-[var(--text-primary)]">{path}</span>
          {dirty && (
            <span className="text-[10.5px] px-1.5 py-0.5 rounded bg-[var(--modified-fg)] text-black font-semibold">
              UNSAVED
            </span>
          )}
          {liveAt > 0 && (
            <span className="flex items-center gap-1.5 text-[10.5px] px-1.5 py-0.5 rounded border border-[var(--added-fg)]/50 text-[var(--added-fg)] font-medium shrink-0">
              <span className="live-dot w-1.5 h-1.5 rounded-full bg-[var(--added-fg)]" />
              Updated on disk
            </span>
          )}
        </div>
        <div className="flex items-center gap-2">
          {isMarkdown && (
            <div className="flex items-center rounded-md border border-[var(--border)] overflow-hidden">
              {(['preview', 'source'] as const).map((m) => (
                <button
                  key={m}
                  onClick={() => switchMode(m)}
                  className={`h-7 px-2.5 text-xs flex items-center gap-1.5 cursor-pointer transition-colors ${
                    mode === m
                      ? 'bg-[var(--bg-active)] text-[var(--text-primary)]'
                      : 'bg-[var(--bg-elevated)] text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                  }`}
                >
                  {m === 'preview' ? <Eye size={13} /> : <Code2 size={13} />}
                  {m === 'preview' ? 'Preview' : 'Source'}
                </button>
              ))}
            </div>
          )}
          <button
            onClick={() => {
              const sel = viewRef.current?.state.sliceDoc(
                viewRef.current.state.selection.main.from,
                viewRef.current.state.selection.main.to
              );
              const snippet = sel?.trim()
                ? `In \`${path}\`:\n\`\`\`\n${sel}\n\`\`\`\n`
                : `Look at \`${path}\` and `;
              onAskAgent(snippet);
            }}
            className="h-7 px-2.5 text-xs rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] flex items-center gap-1.5 cursor-pointer transition-colors"
          >
            <MessageSquarePlus size={13} />
            Ask Agent
          </button>
          {hasDiff && onSwitchToDiff && (
            <button
              onClick={onSwitchToDiff}
              className="h-7 px-2.5 text-xs rounded-md border border-[var(--border)] bg-[var(--bg-elevated)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] flex items-center gap-1.5 cursor-pointer transition-colors"
            >
              <GitCompare size={13} />
              View Diff
            </button>
          )}
          <button
            onClick={handleSave}
            disabled={!dirty || saving}
            className="h-7 px-3 text-xs rounded-md bg-[var(--accent)] text-[var(--accent-fg)] disabled:opacity-40 hover:opacity-90 flex items-center gap-1.5 font-medium cursor-pointer transition-opacity"
          >
            <Save size={13} />
            {saving ? 'Saving...' : 'Save'}
          </button>
        </div>
      </div>
      {dirty && pendingExternal !== null && (
        <div className="flex items-center gap-2 px-3.5 h-8 text-xs border-b border-[var(--border)] bg-[var(--modified-fg)]/15 text-[var(--text-primary)] shrink-0">
          <span className="truncate">This file changed on disk while you have unsaved edits.</span>
          <button
            onClick={reloadFromDisk}
            className="px-2 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] cursor-pointer"
          >
            Reload
          </button>
          <button
            onClick={() => setPendingExternal(null)}
            className="px-2 py-0.5 rounded border border-[var(--border)] hover:bg-[var(--bg-hover)] cursor-pointer"
          >
            Keep mine
          </button>
        </div>
      )}
      {isMarkdown && mode === 'preview' && (
        <div className="flex-1 min-h-0 overflow-auto px-6 py-4 bg-[var(--bg-base)]">
          <div className="max-w-[860px] mx-auto">
            <MarkdownMessage content={previewText} theme={theme} />
          </div>
        </div>
      )}
      <div
        ref={containerRef}
        className={`flex-1 min-h-0 overflow-hidden ${isMarkdown && mode === 'preview' ? 'hidden' : ''}`}
      />
    </div>
  );
};
