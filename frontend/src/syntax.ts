import { javascript } from '@codemirror/lang-javascript';
import { rust } from '@codemirror/lang-rust';
import { python } from '@codemirror/lang-python';
import { json } from '@codemirror/lang-json';
import { css } from '@codemirror/lang-css';
import { html } from '@codemirror/lang-html';
import { markdown, markdownLanguage } from '@codemirror/lang-markdown';
import { yaml } from '@codemirror/lang-yaml';
import { xml } from '@codemirror/lang-xml';
import { sql } from '@codemirror/lang-sql';
import { cpp } from '@codemirror/lang-cpp';
import { java } from '@codemirror/lang-java';
import { go } from '@codemirror/lang-go';
import { toml } from '@codemirror/legacy-modes/mode/toml';
import { shell } from '@codemirror/legacy-modes/mode/shell';
import { powerShell } from '@codemirror/legacy-modes/mode/powershell';
import { dockerFile } from '@codemirror/legacy-modes/mode/dockerfile';
import { ruby } from '@codemirror/legacy-modes/mode/ruby';
import { swift } from '@codemirror/legacy-modes/mode/swift';
import { lua } from '@codemirror/legacy-modes/mode/lua';
import { properties } from '@codemirror/legacy-modes/mode/properties';
import { csharp, kotlin } from '@codemirror/legacy-modes/mode/clike';
import {
  HighlightStyle,
  Language,
  LanguageDescription,
  LanguageSupport,
  StreamLanguage,
  syntaxTree,
  type StreamParser,
} from '@codemirror/language';
import { Prec, RangeSetBuilder, type Extension } from '@codemirror/state';
import { Decoration, ViewPlugin, type DecorationSet, type EditorView, type ViewUpdate } from '@codemirror/view';
import type { SyntaxNodeRef, Tree } from '@lezer/common';
import { highlightTree, tags, type Tag } from '@lezer/highlight';
import type { LiteTheme } from './themes';

/* ------------------------------------------------------------------ */
/* Language registry (single source of truth for editor, diff, chat)   */
/* ------------------------------------------------------------------ */

interface LangDef {
  id: string;
  /** Names usable in markdown fences (```py). */
  aliases: string[];
  /** File extensions without the dot. */
  exts: string[];
  /** Exact lower-cased base names (Dockerfile, Cargo.lock…). */
  files?: string[];
  create: () => LanguageSupport;
}

const stream = (parser: StreamParser<unknown>) => () => new LanguageSupport(StreamLanguage.define(parser));

const LANGS: LangDef[] = [
  { id: 'typescript', aliases: ['ts', 'typescript'], exts: ['ts', 'mts', 'cts'], create: () => javascript({ typescript: true }) },
  { id: 'tsx', aliases: ['tsx'], exts: ['tsx'], create: () => javascript({ typescript: true, jsx: true }) },
  { id: 'javascript', aliases: ['js', 'javascript', 'mjs', 'cjs', 'node'], exts: ['js', 'mjs', 'cjs'], create: () => javascript() },
  { id: 'jsx', aliases: ['jsx'], exts: ['jsx'], create: () => javascript({ jsx: true }) },
  { id: 'rust', aliases: ['rs', 'rust'], exts: ['rs'], create: () => rust() },
  { id: 'python', aliases: ['py', 'python', 'py3', 'python3'], exts: ['py', 'pyi', 'pyw'], create: () => python() },
  {
    id: 'json',
    aliases: ['json', 'jsonc', 'json5'],
    exts: ['json', 'jsonc', 'json5', 'webmanifest', 'map', 'tsbuildinfo'],
    files: ['.prettierrc', '.eslintrc', '.babelrc', '.swcrc', '.stylelintrc'],
    create: () => json(),
  },
  { id: 'css', aliases: ['css', 'scss', 'sass', 'less'], exts: ['css', 'scss', 'sass', 'less'], create: () => css() },
  { id: 'html', aliases: ['html', 'htm', 'xhtml', 'vue', 'svelte'], exts: ['html', 'htm', 'xhtml', 'vue', 'svelte'], create: () => html() },
  { id: 'markdown', aliases: ['md', 'markdown', 'mdx'], exts: ['md', 'markdown', 'mdx'], create: () => markdownSupport() },
  { id: 'yaml', aliases: ['yaml', 'yml'], exts: ['yaml', 'yml'], create: () => yaml() },
  {
    id: 'xml',
    aliases: ['xml', 'svg', 'plist', 'xsl'],
    exts: ['xml', 'svg', 'plist', 'csproj', 'vcxproj', 'xsl', 'xslt', 'xsd', 'xaml', 'props', 'targets', 'resx'],
    create: () => xml(),
  },
  { id: 'sql', aliases: ['sql', 'mysql', 'pgsql', 'postgres', 'sqlite'], exts: ['sql'], create: () => sql() },
  {
    id: 'cpp',
    aliases: ['c', 'h', 'cpp', 'c++', 'cc', 'cxx', 'hpp'],
    exts: ['c', 'h', 'cpp', 'hpp', 'cc', 'cxx', 'hh', 'hxx', 'ino'],
    create: () => cpp(),
  },
  { id: 'java', aliases: ['java'], exts: ['java'], create: () => java() },
  { id: 'go', aliases: ['go', 'golang'], exts: ['go'], files: ['go.mod', 'go.sum'], create: () => go() },
  { id: 'toml', aliases: ['toml'], exts: ['toml'], files: ['cargo.lock', 'poetry.lock'], create: stream(toml) },
  {
    id: 'shell',
    aliases: ['sh', 'bash', 'zsh', 'shell', 'console', 'shellscript', 'env', 'dotenv', 'makefile', 'make'],
    exts: ['sh', 'bash', 'zsh', 'env', 'mk'],
    files: ['makefile', 'gnumakefile', '.gitignore', '.gitattributes', '.dockerignore', '.npmignore', '.bashrc', '.zshrc', '.profile'],
    create: stream(shell),
  },
  { id: 'powershell', aliases: ['ps1', 'powershell', 'pwsh', 'ps'], exts: ['ps1', 'psm1', 'psd1'], create: stream(powerShell) },
  { id: 'dockerfile', aliases: ['dockerfile', 'docker'], exts: ['dockerfile'], files: ['dockerfile'], create: stream(dockerFile) },
  {
    id: 'ini',
    aliases: ['ini', 'conf', 'cfg', 'properties'],
    exts: ['ini', 'conf', 'cfg', 'properties', 'editorconfig'],
    files: ['.editorconfig'],
    create: stream(properties),
  },
  { id: 'ruby', aliases: ['rb', 'ruby'], exts: ['rb', 'rake', 'gemspec'], files: ['gemfile', 'rakefile'], create: stream(ruby) },
  { id: 'csharp', aliases: ['cs', 'csharp', 'c#'], exts: ['cs'], create: stream(csharp) },
  { id: 'kotlin', aliases: ['kt', 'kotlin', 'kts'], exts: ['kt', 'kts'], create: stream(kotlin) },
  { id: 'swift', aliases: ['swift'], exts: ['swift'], create: stream(swift) },
  { id: 'lua', aliases: ['lua'], exts: ['lua'], create: stream(lua) },
];

const BY_EXT = new Map<string, LangDef>();
const BY_FILE = new Map<string, LangDef>();
const BY_NAME = new Map<string, LangDef>();
for (const def of LANGS) {
  for (const e of def.exts) BY_EXT.set(e, def);
  for (const f of def.files ?? []) BY_FILE.set(f, def);
  for (const a of [def.id, ...def.aliases]) BY_NAME.set(a.toLowerCase(), def);
}

const supportCache = new Map<string, LanguageSupport>();
function supportFor(def: LangDef): LanguageSupport {
  let s = supportCache.get(def.id);
  if (!s) {
    s = def.create();
    supportCache.set(def.id, s);
  }
  return s;
}

const descriptionCache = new Map<string, LanguageDescription>();
/** Resolves fenced code block info strings (```python) to a loaded language. */
function resolveCodeLanguage(info: string): LanguageDescription | null {
  const def = BY_NAME.get(info.trim().split(/[\s{,]/)[0].toLowerCase());
  if (!def) return null;
  let desc = descriptionCache.get(def.id);
  if (!desc) {
    desc = LanguageDescription.of({ name: def.id, alias: def.aliases, support: supportFor(def) });
    descriptionCache.set(def.id, desc);
  }
  return desc;
}

function markdownSupport(): LanguageSupport {
  return markdown({ base: markdownLanguage, codeLanguages: resolveCodeLanguage });
}

function findDefByPath(path: string): LangDef | null {
  const base = (path.split(/[\\/]/).pop() ?? '').toLowerCase();
  if (!base) return null;
  const byFile = BY_FILE.get(base);
  if (byFile) return byFile;
  if (base.startsWith('dockerfile')) return BY_NAME.get('dockerfile') ?? null;
  if (base === '.env' || base.startsWith('.env.')) return BY_NAME.get('shell') ?? null;
  const dot = base.lastIndexOf('.');
  return dot >= 0 ? BY_EXT.get(base.slice(dot + 1)) ?? null : null;
}

export function getLanguageSupport(path: string): LanguageSupport | null {
  const def = findDefByPath(path);
  return def ? supportFor(def) : null;
}

export function getLanguageSupportByName(name: string): LanguageSupport | null {
  const def = BY_NAME.get(name.trim().toLowerCase());
  return def ? supportFor(def) : null;
}

export function getLanguageForPath(path: string): Language | null {
  return getLanguageSupport(path)?.language ?? null;
}

export function isMarkdownPath(path: string): boolean {
  return findDefByPath(path)?.id === 'markdown';
}

/** Language id for a path (e.g. 'python'), or null when unsupported. */
export function getLanguageId(path: string): string | null {
  return findDefByPath(path)?.id ?? null;
}

/* ------------------------------------------------------------------ */
/* Token styling                                                       */
/* ------------------------------------------------------------------ */

type PaletteKey = keyof LiteTheme['syntax'];

interface TokenStyleSpec {
  tag: Tag | Tag[];
  key: PaletteKey;
  italic?: boolean;
  bold?: boolean;
  underline?: boolean;
  strike?: boolean;
  /** Overrides font-weight in the CodeMirror editor (diff view uses plain bold). */
  weight?: string;
}

function paletteColor(theme: LiteTheme, key: PaletteKey): string {
  const s = theme.syntax;
  if (key === 'property') return s.property ?? s.variable;
  if (key === 'meta') return s.meta ?? s.type;
  return s[key] as string;
}

// More specific tags (modified / child tags) win over their parents, so order does not matter.
const TOKEN_SPECS: TokenStyleSpec[] = [
  { tag: [tags.keyword, tags.controlKeyword, tags.modifier, tags.definitionKeyword, tags.operatorKeyword, tags.moduleKeyword], key: 'keyword', bold: true, weight: '500' },
  { tag: tags.self, key: 'keyword', italic: true },
  { tag: [tags.string, tags.special(tags.string), tags.regexp, tags.monospace], key: 'string' },
  { tag: [tags.number, tags.bool, tags.null, tags.atom, tags.constant(tags.variableName), tags.constant(tags.propertyName), tags.escape, tags.color], key: 'number' },
  { tag: [tags.comment, tags.lineComment, tags.blockComment, tags.docComment], key: 'comment', italic: true },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.standard(tags.variableName)], key: 'func' },
  { tag: [tags.typeName, tags.className, tags.namespace, tags.tagName], key: 'type' },
  { tag: [tags.propertyName, tags.attributeName, tags.special(tags.variableName)], key: 'property' },
  { tag: [tags.variableName], key: 'variable' },
  { tag: [tags.operator, tags.punctuation, tags.brace, tags.bracket], key: 'operator' },
  // Decorators, attributes, macros and markup markers (#, >, *, `, [ ]).
  { tag: [tags.meta, tags.documentMeta, tags.annotation, tags.processingInstruction, tags.macroName, tags.labelName], key: 'meta' },
  // Markdown structure.
  { tag: tags.heading, key: 'keyword', bold: true, weight: '700' },
  { tag: tags.strong, key: 'keyword', bold: true, weight: '700' },
  { tag: tags.emphasis, key: 'variable', italic: true },
  { tag: tags.strikethrough, key: 'comment', strike: true },
  { tag: [tags.link, tags.url], key: 'func', underline: true },
  { tag: tags.quote, key: 'comment', italic: true },
  { tag: tags.contentSeparator, key: 'operator' },
];

export function buildHighlightStyle(theme: LiteTheme) {
  return HighlightStyle.define(
    TOKEN_SPECS.map((spec) => ({
      tag: spec.tag,
      color: paletteColor(theme, spec.key),
      fontWeight: spec.weight ?? (spec.bold ? 'bold' : undefined),
      fontStyle: spec.italic ? 'italic' : undefined,
      textDecoration: spec.underline ? 'underline' : spec.strike ? 'line-through' : undefined,
    }))
  );
}

export interface TokenSpan {
  text: string;
  color?: string;
  italic?: boolean;
  bold?: boolean;
  underline?: boolean;
  strike?: boolean;
}

interface StyleBits {
  color?: string;
  italic?: boolean;
  bold?: boolean;
  underline?: boolean;
  strike?: boolean;
}

const TOKEN_HIGHLIGHTER = HighlightStyle.define(
  TOKEN_SPECS.map((spec, idx) => ({ tag: spec.tag, class: `t${idx}` }))
);

/**
 * highlightTree reports a space-separated class list when a token sits inside an
 * inherited-mode container (markdown headings, emphasis, quotes, rust attributes…).
 * Parent classes come first; the child's colour wins while weight/style/decoration accumulate.
 */
function mergeClasses(classes: string, theme: LiteTheme, cache: Map<string, StyleBits | null>): StyleBits | null {
  if (cache.has(classes)) return cache.get(classes)!;
  let out: StyleBits | null = null;
  for (const cls of classes.split(' ')) {
    const spec = cls.charCodeAt(0) === 116 /* t */ ? TOKEN_SPECS[Number(cls.slice(1))] : undefined;
    if (!spec) continue;
    out = out ?? {};
    out.color = paletteColor(theme, spec.key);
    if (spec.italic) out.italic = true;
    if (spec.bold) out.bold = true;
    if (spec.underline) out.underline = true;
    if (spec.strike) out.strike = true;
  }
  cache.set(classes, out);
  return out;
}

/* ------------------------------------------------------------------ */
/* Python semantic enrichment                                          */
/* ------------------------------------------------------------------ */
/* @lezer/python tags nearly every identifier as VariableName, so self,
   built-in types, exceptions, dunders and decorators all look alike.
   These overrides refine them by name; shared by the editor and diff view. */

interface Override {
  from: number;
  to: number;
  key: PaletteKey;
  italic?: boolean;
}

const PY_BUILTIN_TYPES = new Set([
  'int', 'str', 'bool', 'float', 'complex', 'list', 'dict', 'set', 'frozenset', 'tuple', 'bytes',
  'bytearray', 'memoryview', 'object', 'type', 'slice', 'property', 'classmethod', 'staticmethod',
]);
const PY_BUILTIN_FUNCS = new Set([
  'len', 'range', 'enumerate', 'zip', 'map', 'filter', 'isinstance', 'issubclass', 'getattr', 'setattr',
  'hasattr', 'delattr', 'super', 'open', 'print', 'input', 'sorted', 'reversed', 'sum', 'min', 'max',
  'abs', 'all', 'any', 'iter', 'next', 'id', 'hash', 'repr', 'format', 'vars', 'dir', 'callable', 'round',
  'divmod', 'chr', 'ord', 'bin', 'hex', 'oct', 'eval', 'exec', 'compile', 'globals', 'locals', 'pow',
  'ascii', 'breakpoint', 'help', 'aiter', 'anext',
]);

function classifyPythonName(name: string, nodeName: string): Pick<Override, 'key' | 'italic'> | null {
  if (nodeName === 'VariableName' && (name === 'self' || name === 'cls')) return { key: 'keyword', italic: true };
  if (/^__\w+__$/.test(name)) return { key: 'func' };
  if (nodeName !== 'VariableName') return null;
  if (PY_BUILTIN_FUNCS.has(name)) return { key: 'func' };
  if (PY_BUILTIN_TYPES.has(name)) return { key: 'type' };
  if (/^[A-Z][A-Z0-9_]+$/.test(name)) return { key: 'number' };
  if (/^[A-Z][A-Za-z0-9_]*$/.test(name)) return { key: 'type' };
  return null;
}

function collectPythonOverrides(tree: Tree, text: (from: number, to: number) => string, from?: number, to?: number): Override[] {
  const out: Override[] = [];
  const visit = (node: SyntaxNodeRef): boolean | void => {
    if (node.name === 'Decorator') {
      tree.iterate({
        from: node.from,
        to: node.to,
        enter(n) {
          if (n.name === 'ArgList') return false;
          if (n.name === 'VariableName' || n.name === 'PropertyName') {
            out.push({ from: n.from, to: n.to, key: 'meta' });
          }
        },
      });
      return false;
    }
    if (node.name === 'VariableName' || node.name === 'PropertyName') {
      const hit = classifyPythonName(text(node.from, node.to), node.name);
      if (hit) out.push({ from: node.from, to: node.to, ...hit });
    }
  };
  tree.iterate({ from, to, enter: visit });
  return out;
}

/** CodeMirror extension that applies the Python name-based overrides on top of the grammar colours. */
export function semanticHighlighting(path: string, theme: LiteTheme): Extension[] {
  if (getLanguageId(path) !== 'python') return [];

  const build = (view: EditorView): DecorationSet => {
    const builder = new RangeSetBuilder<Decoration>();
    const tree = syntaxTree(view.state);
    let last = -1;
    for (const { from, to } of view.visibleRanges) {
      for (const o of collectPythonOverrides(tree, (a, b) => view.state.sliceDoc(a, b), from, to)) {
        if (o.from < last) continue;
        const style = `color:${paletteColor(theme, o.key)} !important;${o.italic ? 'font-style:italic;' : ''}`;
        builder.add(o.from, o.to, Decoration.mark({ attributes: { style } }));
        last = o.to;
      }
    }
    return builder.finish();
  };

  return [
    Prec.highest(
      ViewPlugin.fromClass(
      class {
        decorations: DecorationSet;
        constructor(view: EditorView) {
          this.decorations = build(view);
        }
        update(u: ViewUpdate) {
          if (u.docChanged || u.viewportChanged || syntaxTree(u.startState) !== syntaxTree(u.state)) {
            this.decorations = build(u.view);
          }
        }
      },
      { decorations: (v) => v.decorations }
      )
    ),
  ];
}

/* ------------------------------------------------------------------ */
/* Line tokenizer for diff / chat views                                */
/* ------------------------------------------------------------------ */

function splitPlain(code: string): string[] {
  const lines = code.split(/\r?\n/);
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
  return lines;
}

function highlightWithDef(code: string, def: LangDef | null, theme: LiteTheme): TokenSpan[][] {
  const lines = splitPlain(code);
  if (!def || code.length > 250_000) {
    return lines.map((l) => (l ? [{ text: l }] : []));
  }
  const lang = supportFor(def).language;

  const normalized = lines.join('\n');
  const tree = lang.parser.parse(normalized);

  const overrides =
    def.id === 'python' ? collectPythonOverrides(tree, (a, b) => normalized.slice(a, b)) : [];

  const ranges: Array<{ from: number; to: number; style: StyleBits }> = [];
  const cache = new Map<string, StyleBits | null>();
  let oi = 0;
  const push = (from: number, to: number, style: StyleBits) => {
    while (oi < overrides.length && overrides[oi].to <= from) oi++;
    let pos = from;
    for (let k = oi; k < overrides.length && overrides[k].from < to; k++) {
      const o = overrides[k];
      const s = Math.max(o.from, from);
      const e = Math.min(o.to, to);
      if (s > pos) ranges.push({ from: pos, to: s, style });
      ranges.push({
        from: s,
        to: e,
        style: { ...style, color: paletteColor(theme, o.key), italic: o.italic ?? style.italic },
      });
      pos = e;
    }
    if (pos < to) ranges.push({ from: pos, to, style });
  };

  highlightTree(tree, TOKEN_HIGHLIGHTER, (from, to, classes) => {
    const style = mergeClasses(classes, theme, cache);
    if (style) push(from, to, style);
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
        spans.push({
          text: normalized.slice(segStart, segEnd),
          color: r.style.color,
          italic: r.style.italic,
          bold: r.style.bold,
          underline: r.style.underline,
          strike: r.style.strike,
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

/**
 * Tokenize a full document into per-line syntax-highlighted spans using Lezer.
 * Fast enough for instant diff rendering and keeps multi-line comments/strings accurate.
 */
export function highlightLines(code: string, path: string, theme: LiteTheme): TokenSpan[][] {
  return highlightWithDef(code, findDefByPath(path), theme);
}

/** Same as highlightLines, but resolves the language from a fence name (```py). Null when unknown. */
export function highlightLinesByName(code: string, name: string, theme: LiteTheme): TokenSpan[][] | null {
  const def = BY_NAME.get(name.trim().toLowerCase());
  return def ? highlightWithDef(code, def, theme) : null;
}
