import React, { useEffect, useRef, useState } from 'react';
import { EditorState, Extension } from '@codemirror/state';
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter } from '@codemirror/view';
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
}) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
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

    const state = EditorState.create({
      doc: content,
      extensions: [
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
          if (u.docChanged) {
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
    setDirty(false);

    if (jumpToLine) jumpTo(view, jumpToLine, jumpToColumn, jumpToEndColumn);

    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // Recreate when file path, source content, or theme changes
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, content, theme, binary]);

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
