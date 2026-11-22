import { javascriptLanguage, jsxLanguage, tsxLanguage, typescriptLanguage } from '@codemirror/lang-javascript';
import { rustLanguage } from '@codemirror/lang-rust';
import { pythonLanguage } from '@codemirror/lang-python';
import { jsonLanguage } from '@codemirror/lang-json';
import { cssLanguage } from '@codemirror/lang-css';
import { htmlLanguage } from '@codemirror/lang-html';
import { markdownLanguage } from '@codemirror/lang-markdown';
import { HighlightStyle, Language } from '@codemirror/language';
import { highlightTree, tags } from '@lezer/highlight';
import type { LiteTheme } from './themes';

export function getLanguageForPath(path: string): Language | null {
  const lower = path.toLowerCase();
  if (lower.endsWith('.tsx')) return tsxLanguage;
  if (lower.endsWith('.ts') || lower.endsWith('.mts') || lower.endsWith('.cts')) {
    return typescriptLanguage;
  }
  if (lower.endsWith('.jsx')) return jsxLanguage;
  if (lower.endsWith('.js') || lower.endsWith('.mjs') || lower.endsWith('.cjs')) {
    return javascriptLanguage;
  }
  if (lower.endsWith('.rs')) return rustLanguage;
  if (lower.endsWith('.py')) return pythonLanguage;
  if (lower.endsWith('.json')) return jsonLanguage;
  if (lower.endsWith('.css')) return cssLanguage;
  if (lower.endsWith('.html') || lower.endsWith('.htm')) return htmlLanguage;
  if (lower.endsWith('.md') || lower.endsWith('.markdown')) return markdownLanguage;
  return null;
}

export function buildHighlightStyle(theme: LiteTheme) {
  return HighlightStyle.define([
    { tag: [tags.keyword, tags.controlKeyword, tags.modifier, tags.definitionKeyword], color: theme.syntax.keyword, fontWeight: '500' },
    { tag: [tags.string, tags.special(tags.string), tags.regexp], color: theme.syntax.string },
    { tag: [tags.number, tags.bool, tags.null, tags.atom], color: theme.syntax.number },
    { tag: [tags.comment, tags.lineComment, tags.blockComment], color: theme.syntax.comment, fontStyle: 'italic' },
    { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], color: theme.syntax.func },
    { tag: [tags.typeName, tags.className, tags.namespace, tags.tagName], color: theme.syntax.type },
    { tag: [tags.variableName, tags.propertyName, tags.attributeName], color: theme.syntax.variable },
    { tag: [tags.operator, tags.punctuation, tags.brace, tags.bracket], color: theme.syntax.operator },
  ]);
}

export interface TokenSpan {
  text: string;
  color?: string;
  italic?: boolean;
  bold?: boolean;
}

const TOKEN_CLASSES: Array<{ tag: any; key: keyof LiteTheme['syntax']; italic?: boolean; bold?: boolean }> = [
  { tag: [tags.keyword, tags.controlKeyword, tags.modifier, tags.definitionKeyword], key: 'keyword', bold: true },
  { tag: [tags.string, tags.special(tags.string), tags.regexp], key: 'string' },
  { tag: [tags.number, tags.bool, tags.null, tags.atom], key: 'number' },
  { tag: [tags.comment, tags.lineComment, tags.blockComment], key: 'comment', italic: true },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], key: 'func' },
  { tag: [tags.typeName, tags.className, tags.namespace, tags.tagName], key: 'type' },
  { tag: [tags.variableName, tags.propertyName, tags.attributeName], key: 'variable' },
  { tag: [tags.operator, tags.punctuation], key: 'operator' },
];

const TOKEN_HIGHLIGHTER = HighlightStyle.define(
  TOKEN_CLASSES.map((item, idx) => ({
    tag: item.tag,
    class: String(idx),
  }))
);

/**
 * Tokenize a full document into per-line syntax-highlighted spans using Lezer.
 * Fast enough for instant diff rendering and keeps multi-line comments/strings accurate.
 */
export function highlightLines(
  code: string,
  path: string,
  theme: LiteTheme
): TokenSpan[][] {
  const lines = code.split(/\r?\n/);
  if (lines.length > 0 && lines[lines.length - 1] === '') {
    lines.pop();
  }
  const lang = getLanguageForPath(path);
  if (!lang || code.length > 250_000) {
    return lines.map((l) => (l ? [{ text: l }] : []));
  }

  const highlighter = TOKEN_HIGHLIGHTER;

  // Build line start offsets for fast slicing
  const normalized = lines.join('\n');
  const tree = lang.parser.parse(normalized);

  // Collect flat styled ranges across normalized text
  const ranges: Array<{ from: number; to: number; styleIdx: number }> = [];
  highlightTree(tree, highlighter, (from, to, classes) => {
    const idx = Number(classes);
    if (!Number.isNaN(idx)) {
      ranges.push({ from, to, styleIdx: idx });
    }
  });

  const result: TokenSpan[][] = [];
  let offset = 0;
  let rIdx = 0;

  for (const line of lines) {
    const lineStart = offset;
    const lineEnd = offset + line.length;
    offset = lineEnd + 1; // +1 for '\n'

    while (rIdx < ranges.length && ranges[rIdx].to <= lineStart) {
      rIdx++;
    }

    const spans: TokenSpan[] = [];
    let cursor = lineStart;
    let j = rIdx;

    while (j < ranges.length && ranges[j].from < lineEnd) {
      const r = ranges[j];
      const segStart = Math.max(cursor, r.from);
      const segEnd = Math.min(lineEnd, r.to);

      if (segStart > cursor) {
        spans.push({ text: normalized.slice(cursor, segStart) });
      }
      if (segEnd > segStart) {
        const spec = TOKEN_CLASSES[r.styleIdx];
        spans.push({
          text: normalized.slice(segStart, segEnd),
          color: theme.syntax[spec.key],
          italic: spec.italic,
          bold: spec.bold,
        });
        cursor = segEnd;
      }
      if (r.to <= lineEnd) {
        j++;
      } else {
        break;
      }
    }

    if (cursor < lineEnd) {
      spans.push({ text: normalized.slice(cursor, lineEnd) });
    }
    result.push(spans);
  }

  return result;
}
