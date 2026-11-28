import React from 'react';
import { openGotoLine, useEditorStatus } from '../editorStatus';

const GOTO_KEY =
  typeof navigator !== 'undefined' && /Macintosh|Mac OS X/i.test(navigator.userAgent) ? '⌘G' : 'Ctrl+G';

/** "Ln 42, Col 18 (3 lines, 120 selected) · TypeScript" for the focused code editor. */
export const EditorStatusItem: React.FC<{ path: string }> = ({ path }) => {
  const s = useEditorStatus();
  if (!s || s.path !== path) return null;

  let sel = '';
  if (s.cursors > 1) sel = `${s.cursors} selections`;
  else if (s.selChars > 0) sel = s.selLines > 1 ? `${s.selLines} lines, ${s.selChars} selected` : `${s.selChars} selected`;

  return (
    <div className="sb-group flex items-stretch">
      <button
        type="button"
        onClick={openGotoLine}
        className="flex items-center gap-1 px-2 tabular-nums status-item"
        title={`Go to line (${GOTO_KEY})`}
      >
        Ln {s.line}, Col {s.col}
        {sel && <span className="opacity-70">({sel})</span>}
      </button>
      <span className="flex items-center px-2">{s.language}</span>
    </div>
  );
};
