import React, { useEffect, useRef, useState } from 'react';
import { EditorState, Extension } from '@codemirror/state';
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter } from '@codemirror/view';
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { tags } from '@lezer/highlight';
import { javascript } from '@codemirror/lang-javascript';
import { rust } from '@codemirror/lang-rust';
import { python } from '@codemirror/lang-python';
import { json } from '@codemirror/lang-json';
import { css } from '@codemirror/lang-css';
import { html } from '@codemirror/lang-html';
import { markdown } from '@codemirror/lang-markdown';
import { GitCompare, MessageSquarePlus, Save } from 'lucide-react';
import type { LiteTheme } from '../themes';

interface CodeEditorProps {
  path: string;
  content: string;
  binary?: boolean;
  theme: LiteTheme;
  hasDiff?: boolean;
  jumpToLine?: number;
  onSave: (newContent: string) => Promise<void>;
  onSwitchToDiff?: () => void;
  onAskAgent: (snippet: string) => void;
}

function langExtension(path: string): Extension[] {
  const lower = path.toLowerCase();
  if (lower.endsWith('.ts') || lower.endsWith('.tsx')) {
    return [javascript({ typescript: true, jsx: lower.endsWith('.tsx') })];
  }
  if (lower.endsWith('.js') || lower.endsWith('.jsx') || lower.endsWith('.mjs')) {
    return [javascript({ jsx: lower.endsWith('.jsx') })];
  }
  if (lower.endsWith('.rs')) return [rust()];
  if (lower.endsWith('.py')) return [python()];
  if (lower.endsWith('.json')) return [json()];
  if (lower.endsWith('.css')) return [css()];
  if (lower.endsWith('.html') || lower.endsWith('.htm')) return [html()];
  if (lower.endsWith('.md') || lower.endsWith('.markdown')) return [markdown()];
  return [];
}

function buildHighlightStyle(theme: LiteTheme) {
  return HighlightStyle.define([
    { tag: [tags.keyword, tags.controlKeyword, tags.modifier], color: theme.syntax.keyword },
    { tag: [tags.string, tags.special(tags.string)], color: theme.syntax.string },
    { tag: [tags.number, tags.bool, tags.null], color: theme.syntax.number },
    { tag: [tags.comment, tags.lineComment, tags.blockComment], color: theme.syntax.comment, fontStyle: 'italic' },
    { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], color: theme.syntax.func },
    { tag: [tags.typeName, tags.className, tags.namespace], color: theme.syntax.type },
    { tag: [tags.variableName, tags.propertyName], color: theme.syntax.variable },
    { tag: [tags.operator, tags.punctuation], color: theme.syntax.operator },
  ]);
}

export const CodeEditor: React.FC<CodeEditorProps> = ({
  path,
  content,
  binary,
  theme,
  hasDiff,
  jumpToLine,
  onSave,
  onSwitchToDiff,
  onAskAgent,
}) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);

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
        ...langExtension(path),
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

    if (jumpToLine && jumpToLine > 0 && jumpToLine <= state.doc.lines) {
      const lineInfo = state.doc.line(jumpToLine);
      view.dispatch({
        selection: { anchor: lineInfo.from },
        effects: EditorView.scrollIntoView(lineInfo.from, { y: 'center' }),
      });
    }

    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // Recreate when file path, source content, or theme changes
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, content, theme, binary]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || !jumpToLine || jumpToLine < 1 || jumpToLine > view.state.doc.lines) return;
    const lineInfo = view.state.doc.line(jumpToLine);
    view.dispatch({
      selection: { anchor: lineInfo.from },
      effects: EditorView.scrollIntoView(lineInfo.from, { y: 'center' }),
    });
  }, [jumpToLine]);

  if (binary) {
    return (
      <div className="flex-1 flex items-center justify-center text-[var(--text-muted)]">
        Binary file cannot be displayed in the text editor.
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col min-h-0">
      <div className="flex items-center justify-between px-3 py-1.5 border-b border-[var(--border)] bg-[var(--bg-sidebar)] shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          <span className="font-mono text-xs truncate text-[var(--text-primary)]">{path}</span>
          {dirty && (
            <span className="text-[10px] px-1.5 py-0.5 rounded bg-[var(--modified-fg)] text-black font-semibold">
              UNSAVED
            </span>
          )}
        </div>
        <div className="flex items-center gap-1.5">
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
            className="px-2 py-1 text-xs rounded border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
          >
            <MessageSquarePlus size={13} />
            Ask Agent
          </button>
          {hasDiff && onSwitchToDiff && (
            <button
              onClick={onSwitchToDiff}
              className="px-2 py-1 text-xs rounded border border-[var(--border)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] flex items-center gap-1 cursor-pointer"
            >
              <GitCompare size={13} />
              View Diff
            </button>
          )}
          <button
            onClick={handleSave}
            disabled={!dirty || saving}
            className="px-2.5 py-1 text-xs rounded bg-[var(--accent)] text-[var(--accent-fg)] disabled:opacity-40 hover:opacity-90 flex items-center gap-1 cursor-pointer"
          >
            <Save size={13} />
            {saving ? 'Saving...' : 'Save'}
          </button>
        </div>
      </div>
      <div ref={containerRef} className="flex-1 min-h-0 overflow-hidden" />
    </div>
  );
};
