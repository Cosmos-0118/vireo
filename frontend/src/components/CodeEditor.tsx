import { Binary } from '@phosphor-icons/react';
import { EmptyState } from './ui/EmptyState';
import React, { Suspense, useEffect, useRef, useState } from 'react';
import {
  Annotation,
  Compartment,
  EditorState,
  Extension,
  RangeSet,
  RangeSetBuilder,
  StateEffect,
  StateField,
} from '@codemirror/state';
import {
  Decoration,
  DecorationSet,
  EditorView,
  GutterMarker,
  ViewPlugin,
  WidgetType,
  gutter,
  keymap,
  lineNumbers,
  highlightActiveLine,
  highlightActiveLineGutter,
} from '@codemirror/view';
import { diffLines, diffWordsWithSpace } from 'diff';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import {
  closeSearchPanel,
  findNext,
  findPrevious,
  gotoLine,
  highlightSelectionMatches,
  openSearchPanel,
  search,
  selectNextOccurrence,
} from '@codemirror/search';
import { syntaxHighlighting } from '@codemirror/language';
import {
  Check,
  ChevronDown,
  ChevronUp,
  Code2,
  Eye,
  GitCompare,
  MessageSquarePlus,
  RotateCcw,
  Save,
} from 'lucide-react';
import {
  buildHighlightStyle,
  getLanguageSupport,
  isMarkdownPath,
  loadLanguageForPath,
  semanticHighlighting,
} from '../syntax';

// react-markdown + rehype-sanitize are heavy; only fetch them when a markdown preview opens.
const MarkdownMessage = React.lazy(() => import('./MarkdownView').then((m) => ({ default: m.MarkdownMessage })));
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
  /** Content before the agent's unreviewed edits; changes against it are shown inline. */
  reviewBase?: string | null;
  /** New baseline after Keep (string) or review finished (null). */
  onReviewBaseChange?: (base: string | null) => void;
  /** "Undo file" ran: the content it replaced and the baseline it restored (for Redo). */
  onFileUndone?: (before: string, base: string) => void;
  /** True until `content` holds this file's real text (it is '' meanwhile). */
  loading?: boolean;
  /**
   * An agent edit to reveal: flash `ranges` (1-based new-doc lines) and scroll to `line`
   * (without stealing focus). Scrolling follows `followEdits` and pauses while the user is busy,
   * unless `reveal` is set (the user asked for it, e.g. clicked the file in the edits bar).
   */
  scrollTarget?: {
    key: number;
    line: number;
    ranges?: Array<{ from: number; to: number }>;
    reveal?: boolean;
    /** Text (LF) right after the edit; when given, wait until the editor shows exactly this. */
    text?: string;
  } | null;
}

interface ReviewHunk {
  oldFrom: number;
  oldTo: number;
  newFrom: number;
  newTo: number;
  removed: string;
  added: string;
  /** Changed word ranges inside `removed` / `added` (offsets relative to those strings). */
  delWords?: Array<[number, number]>;
  addWords?: Array<[number, number]>;
}

/** Word diffs only for small modified hunks, where they help instead of painting everything. */
const WORD_DIFF_MAX_CHARS = 4000;
const WORD_DIFF_MAX_LINES = 40;

function lineCount(t: string) {
  let n = 0;
  for (let i = 0; i < t.length; i++) if (t.charCodeAt(i) === 10) n++;
  return t.endsWith('\n') ? n : n + 1;
}

function addWordDiff(h: ReviewHunk) {
  const { removed, added } = h;
  if (!removed || !added) return;
  if (removed.length > WORD_DIFF_MAX_CHARS || added.length > WORD_DIFF_MAX_CHARS) return;
  if (lineCount(removed) > WORD_DIFF_MAX_LINES || lineCount(added) > WORD_DIFF_MAX_LINES) return;
  const del: Array<[number, number]> = [];
  const add: Array<[number, number]> = [];
  let ro = 0;
  let ao = 0;
  let changed = 0;
  for (const part of diffWordsWithSpace(removed, added)) {
    const len = part.value.length;
    if (part.added) {
      if (part.value.trim()) add.push([ao, ao + len]);
      changed += len;
      ao += len;
    } else if (part.removed) {
      if (part.value.trim()) del.push([ro, ro + len]);
      ro += len;
    } else {
      ro += len;
      ao += len;
    }
  }
  // Mostly rewritten: word marks would just repaint the whole hunk.
  if (changed > added.length * 0.6) return;
  h.delWords = del;
  h.addWords = add;
}

interface ReviewHandlers {
  keep: (view: EditorView, h: ReviewHunk) => void;
  undo: (view: EditorView, h: ReviewHunk) => void;
}

const setReviewBase = StateEffect.define<string | null>();
/** Debounced "re-diff against the base now" for big documents (see makeReviewField). */
const recomputeReview = StateEffect.define<null>();
/** Above this combined size a keystroke must not trigger a synchronous full-document diff. */
const SYNC_REVIEW_DIFF_LIMIT = 60_000;

function computeReviewHunks(base: string, doc: string): ReviewHunk[] {
  if (base.length + doc.length > 600000) return [];
  const hunks: ReviewHunk[] = [];
  let o = 0;
  let n = 0;
  let cur: ReviewHunk | null = null;
  for (const part of diffLines(base, doc)) {
    const len = part.value.length;
    if (part.added || part.removed) {
      if (!cur) cur = { oldFrom: o, oldTo: o, newFrom: n, newTo: n, removed: '', added: '' };
      if (part.added) {
        cur.added += part.value;
        n += len;
        cur.newTo = n;
      } else {
        cur.removed += part.value;
        o += len;
        cur.oldTo = o;
      }
    } else {
      if (cur) {
        hunks.push(cur);
        cur = null;
      }
      o += len;
      n += len;
    }
  }
  if (cur) hunks.push(cur);
  for (const h of hunks) addWordDiff(h);
  return hunks;
}

/** Deleted lines shown inline before "Show N more" (big deletions must not take over the view). */
const DEL_LINES_SHOWN = 12;
/** Approximate row height (14.5px font x 1.65 line-height), for CodeMirror's height estimates. */
const ROW_PX = 24;
const INS_HEADER_PX = 22;

/** One deleted line, with its changed words marked. `start` is its offset inside `removed`. */
function renderDelLine(text: string, start: number, words: Array<[number, number]> | undefined) {
  const l = document.createElement('div');
  l.className = 'cm-review-del-line';
  if (!text) {
    l.textContent = ' ';
    return l;
  }
  const end = start + text.length;
  let pos = start;
  for (const [a, b] of words ?? []) {
    if (b <= pos || a >= end) continue;
    const s = Math.max(a, pos);
    const e = Math.min(b, end);
    if (s > pos) l.append(text.slice(pos - start, s - start));
    const span = document.createElement('span');
    span.className = 'cm-review-word-del';
    span.textContent = text.slice(s - start, e - start);
    l.append(span);
    pos = e;
  }
  if (pos < end) l.append(text.slice(pos - start));
  return l;
}

class ReviewWidget extends WidgetType {
  constructor(
    readonly h: ReviewHunk,
    readonly handlers: { current: ReviewHandlers | null }
  ) {
    super();
  }
  eq(o: ReviewWidget) {
    return (
      o.h.oldFrom === this.h.oldFrom &&
      o.h.newFrom === this.h.newFrom &&
      o.h.removed === this.h.removed &&
      o.h.added === this.h.added
    );
  }
  get estimatedHeight() {
    if (!this.h.removed) return INS_HEADER_PX;
    const n = lineCount(this.h.removed);
    return Math.min(n, DEL_LINES_SHOWN) * ROW_PX + (n > DEL_LINES_SHOWN ? INS_HEADER_PX : 0);
  }
  toDOM(view: EditorView) {
    const wrap = document.createElement('div');
    if (this.h.removed) {
      wrap.className = 'cm-review-widget';
      const del = document.createElement('div');
      del.className = 'cm-review-del';
      const lines = this.h.removed.replace(/\n$/, '').split('\n');
      const starts: number[] = [];
      for (let i = 0, off = 0; i < lines.length; i++) {
        starts.push(off);
        off += lines[i].length + 1;
      }
      const words = this.h.delWords;
      const shown = lines.length > DEL_LINES_SHOWN + 2 ? DEL_LINES_SHOWN : lines.length;
      for (let i = 0; i < shown; i++) del.append(renderDelLine(lines[i], starts[i], words));
      if (shown < lines.length) {
        const more = document.createElement('button');
        more.className = 'cm-review-more';
        more.textContent = `Show ${lines.length - shown} more deleted lines`;
        more.addEventListener('mousedown', (e) => e.preventDefault());
        more.addEventListener('click', (e) => {
          e.preventDefault();
          const frag = document.createDocumentFragment();
          for (let i = shown; i < lines.length; i++) frag.append(renderDelLine(lines[i], starts[i], words));
          more.replaceWith(frag);
          view.requestMeasure();
        });
        del.append(more);
      }
      wrap.append(del);
    } else {
      // Pure insertion: an in-flow header row, so the buttons never cover the inserted code.
      wrap.className = 'cm-review-widget cm-review-ins';
      const label = document.createElement('span');
      label.className = 'cm-review-ins-label';
      const n = lineCount(this.h.added);
      label.textContent = `+${n} line${n === 1 ? '' : 's'}`;
      wrap.append(label);
    }
    const pill = document.createElement('div');
    pill.className = 'cm-review-pill';
    const mk = (label: string, title: string, run: () => void) => {
      const b = document.createElement('button');
      b.textContent = label;
      b.title = title;
      b.addEventListener('mousedown', (e) => e.preventDefault());
      b.addEventListener('click', (e) => {
        e.preventDefault();
        run();
      });
      pill.append(b);
    };
    mk('\u2713 Keep', 'Accept this change', () => this.handlers.current?.keep(view, this.h));
    mk('\u21ba Undo', 'Revert this change on disk', () => this.handlers.current?.undo(view, this.h));
    wrap.append(pill);
    return wrap;
  }
  ignoreEvent() {
    return true;
  }
}

class ReviewGutterMarker extends GutterMarker {
  readonly elementClass: string;
  constructor(readonly kind: 'add' | 'mod' | 'del') {
    super();
    this.elementClass = `cm-review-gutter-${kind}`;
  }
  eq(o: ReviewGutterMarker) {
    return o.kind === this.kind;
  }
}
const gutterAdd = new ReviewGutterMarker('add');
const gutterMod = new ReviewGutterMarker('mod');
const gutterDel = new ReviewGutterMarker('del');
const reviewAddLine = Decoration.line({ class: 'cm-review-add' });
const reviewWordAdd = Decoration.mark({ class: 'cm-review-word-add' });
const agentFlashLine = Decoration.line({ class: 'cm-agent-flash' });

type ReviewState = {
  base: string | null;
  hunks: ReviewHunk[];
  deco: DecorationSet;
  gutter: RangeSet<GutterMarker>;
};

/** Start line (1-based) of each hunk in the current document. */
function hunkLines(state: EditorState, hunks: ReviewHunk[]) {
  return hunks.map((h) => state.doc.lineAt(Math.min(h.newFrom, state.doc.length)).number);
}

function makeReviewField(handlers: { current: ReviewHandlers | null }) {
  return StateField.define<ReviewState>({
    create: () => ({ base: null, hunks: [], deco: Decoration.none, gutter: RangeSet.empty }),
    update(v, tr) {
      let base = v.base;
      let changed = tr.docChanged;
      let forced = false; // base changed or a debounced recompute landed
      for (const e of tr.effects) {
        if (e.is(setReviewBase)) {
          base = e.value === null ? null : e.value.replace(/\r\n?/g, '\n');
          changed = true;
          forced = true;
        } else if (e.is(recomputeReview)) {
          changed = true;
          forced = true;
        }
      }
      if (!changed) return v;
      if (base === null) return { base, hunks: [], deco: Decoration.none, gutter: RangeSet.empty };
      const doc = tr.state.doc;
      if (!forced && base.length + doc.length > SYNC_REVIEW_DIFF_LIMIT) {
        // Big file while typing: keep the previous hunks (re-anchored) until the debounced
        // recompute runs, instead of diffing the whole document on every keystroke.
        const map = (pos: number, assoc: -1 | 1) => tr.changes.mapPos(pos, assoc);
        return {
          base,
          hunks: v.hunks.map((h) => ({ ...h, newFrom: map(h.newFrom, -1), newTo: map(h.newTo, 1) })),
          deco: v.deco.map(tr.changes),
          gutter: v.gutter.map(tr.changes),
        };
      }
      const hunks = computeReviewHunks(base, doc.toString());
      const ranges = [];
      const marks = new RangeSetBuilder<GutterMarker>();
      for (const h of hunks) {
        const start = doc.lineAt(Math.min(h.newFrom, doc.length));
        ranges.push(
          Decoration.widget({ widget: new ReviewWidget(h, handlers), block: true, side: -1 }).range(
            start.from
          )
        );
        if (h.added) {
          const last = doc.lineAt(Math.min(Math.max(h.newFrom, h.newTo - 1), doc.length));
          const marker = h.removed ? gutterMod : gutterAdd;
          for (let l = start.number; l <= last.number && l - start.number < 800; l++) {
            const from = doc.line(l).from;
            ranges.push(reviewAddLine.range(from));
            marks.add(from, from, marker);
          }
          for (const [a, b] of h.addWords ?? []) {
            const from = Math.min(h.newFrom + a, doc.length);
            const to = Math.min(h.newFrom + b, doc.length);
            if (to > from) ranges.push(reviewWordAdd.range(from, to));
          }
        } else {
          marks.add(start.from, start.from, gutterDel);
        }
      }
      return { base, hunks, deco: Decoration.set(ranges, true), gutter: marks.finish() };
    },
    provide: (f) => EditorView.decorations.from(f, (v) => v.deco),
  });
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
          e.value.map((pos) => agentFlashLine.range(pos)),
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
  reviewBase,
  onReviewBaseChange,
  onFileUndone,
  scrollTarget,
  loading,
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
  const reviewHandlersRef = useRef<ReviewHandlers | null>(null);
  const reviewFieldRef = useRef<StateField<ReviewState> | null>(null);
  const reviewBaseRef = useRef(reviewBase);
  reviewBaseRef.current = reviewBase;
  const onReviewBaseChangeRef = useRef(onReviewBaseChange);
  onReviewBaseChangeRef.current = onReviewBaseChange;
  const onFileUndoneRef = useRef(onFileUndone);
  onFileUndoneRef.current = onFileUndone;
  const [reviewCount, setReviewCount] = useState(0);
  /** Hunk start lines + doc length, for the overview ruler and the change stepper. */
  const [reviewMarks, setReviewMarks] = useState<{
    lines: Array<{ line: number; span: number; kind: 'add' | 'mod' | 'del' }>;
    total: number;
  }>({ lines: [], total: 1 });
  const [navIdx, setNavIdx] = useState(-1);
  /** The view has shown at least one hunk since its baseline was set (see the auto-clear). */
  const hadHunksRef = useRef(false);
  /** The view holds this file's real text (not the '' placeholder shown while loading). */
  const readyRef = useRef(false);
  const flashTimerRef = useRef(0);
  const navRef = useRef<(dir: 1 | -1) => boolean>(() => false);

  /** The agent-edit glow is one finite animation; drop its decorations once it has played. */
  const armFlashClear = () => {
    window.clearTimeout(flashTimerRef.current);
    flashTimerRef.current = window.setTimeout(() => {
      viewRef.current?.dispatch({ effects: setFlash.of([]) });
    }, 2100);
  };
  useEffect(() => () => window.clearTimeout(flashTimerRef.current), []);

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
  // Markdown opens rendered, except for a jump-to-line or unreviewed agent edits (the preview
  // would hide the inline review and its Keep / Undo buttons).
  const initialMode = (): 'source' | 'preview' =>
    isMarkdown && !jumpToLine && reviewBase == null ? 'preview' : 'source';
  const [mode, setMode] = useState<'source' | 'preview'>(initialMode);
  const modeRef = useRef(mode);
  modeRef.current = mode;
  const [previewText, setPreviewText] = useState(content);

  useEffect(() => {
    setMode(initialMode());
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

  // Ctrl+S is a global shortcut (see keybindings.ts), so it saves even when focus is outside the editor.
  const handleSaveRef = useRef(handleSave);
  handleSaveRef.current = handleSave;
  useEffect(() => {
    const onSave = () => void handleSaveRef.current();
    window.addEventListener('vireo:save-file', onSave);
    return () => window.removeEventListener('vireo:save-file', onSave);
  }, []);

  useEffect(() => {
    if (!containerRef.current || binary) return;

    // A theme change recreates the view; carry unsaved edits across instead of reloading from disk.
    const carried = carryRef.current && carryRef.current.path === path ? carryRef.current : null;
    carryRef.current = null;

    const reviewField = makeReviewField(reviewHandlersRef);
    reviewFieldRef.current = reviewField;
    const reviewDebounce = ViewPlugin.fromClass(
      class {
        timer = 0;
        constructor(readonly view: EditorView) {}
        update(u: { docChanged: boolean }) {
          if (!u.docChanged) return;
          const rv = this.view.state.field(reviewField, false);
          if (!rv || rv.base === null) return;
          window.clearTimeout(this.timer);
          this.timer = window.setTimeout(() => {
            this.view.dispatch({ effects: recomputeReview.of(null) });
          }, 200);
        }
        destroy() {
          window.clearTimeout(this.timer);
        }
      }
    );

    const langCompartment = new Compartment();
    let disposed = false;
    const state = EditorState.create({
      doc: carried ? carried.doc : normalizeNewlines(contentRef.current),
      selection: carried ? { anchor: Math.min(carried.anchor, carried.doc.length) } : undefined,
      extensions: [
        flashField,
        reviewField,
        reviewDebounce,
        lineNumbers(),
        gutter({
          class: 'cm-review-gutter',
          markers: (v) => v.state.field(reviewField).gutter,
        }),
        highlightActiveLine(),
        highlightActiveLineGutter(),
        history(),
        search({ top: true }),
        highlightSelectionMatches(),
        keymap.of([
          { key: 'Alt-]', run: () => navRef.current(1) },
          { key: 'Alt-[', run: () => navRef.current(-1) },
          // Mod-Alt-g (CodeMirror's default Go to line) is the global Git Graph shortcut, so Go to line is Mod-g.
          { key: 'Mod-f', run: openSearchPanel, scope: 'editor search-panel' },
          { key: 'Mod-h', run: openSearchPanel, scope: 'editor search-panel' },
          { key: 'F3', run: findNext, shift: findPrevious, scope: 'editor search-panel' },
          { key: 'Mod-g', run: gotoLine },
          { key: 'Mod-d', run: selectNextOccurrence, preventDefault: true },
          { key: 'Escape', run: closeSearchPanel },
          indentWithTab,
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        langCompartment.of(langExtension(path, theme)),
        syntaxHighlighting(buildHighlightStyle(theme)),
        EditorView.updateListener.of((u) => {
          if (u.docChanged && !u.transactions.some((t) => t.annotation(externalChange))) {
            setDirty(true);
          }
          const rv = u.state.field(reviewField);
          if (
            u.docChanged ||
            u.transactions.some((t) => t.effects.some((e) => e.is(setReviewBase) || e.is(recomputeReview)))
          ) {
            setReviewCount(rv.hunks.length);
            const starts = hunkLines(u.state, rv.hunks);
            const lines = rv.hunks.map((h, i) => ({
              line: starts[i],
              span: h.added ? Math.max(1, lineCount(h.added)) : 1,
              kind: (!h.added ? 'del' : h.removed ? 'mod' : 'add') as 'add' | 'mod' | 'del',
            }));
            const total = u.state.doc.lines;
            // Same marks: keep the old object so a keystroke doesn't re-render the editor.
            setReviewMarks((prev) =>
              (lines.length === 0 && prev.lines.length === 0) ||
              (prev.total === total &&
                prev.lines.length === lines.length &&
                prev.lines.every(
                  (m, i) => m.line === lines[i].line && m.span === lines[i].span && m.kind === lines[i].kind
                ))
                ? prev
                : { total, lines }
            );
            // Review finished: every hunk was kept or undone. Only on a real "had hunks -> none"
            // transition: a baseline applied to stale or not-yet-loaded content can produce zero
            // hunks for a moment, and must not wipe the review before the fresh text arrives.
            if (rv.base !== null && rv.hunks.length === 0 && hadHunksRef.current) {
              queueMicrotask(() => onReviewBaseChangeRef.current?.(null));
            }
            hadHunksRef.current = rv.base !== null && rv.hunks.length > 0;
          }
        }),
      ],
    });

    const view = new EditorView({
      state,
      parent: containerRef.current,
    });
    viewRef.current = view;
    // Grammars load on demand: paint plain text now, then attach the language once its chunk arrives.
    if (!getLanguageSupport(path)) {
      void loadLanguageForPath(path).then(() => {
        if (disposed || viewRef.current !== view) return;
        view.dispatch({ effects: langCompartment.reconfigure(langExtension(path, theme)) });
      });
    }
    hadHunksRef.current = false;
    readyRef.current = !!carried || !loading;
    setNavIdx(-1);
    if (readyRef.current && reviewBaseRef.current != null) {
      view.dispatch({ effects: setReviewBase.of(reviewBaseRef.current) });
    }
    if (!carried) setDirty(false);
    setPendingExternal(null);

    if (jumpToLine && readyRef.current) jumpTo(view, jumpToLine, jumpToColumn, jumpToEndColumn);

    return () => {
      disposed = true;
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
    if (!view || binary || loading) return;
    const next = normalizeNewlines(content);
    const cur = view.state.doc.toString();
    if (!readyRef.current) {
      // First real text for a view created while loading: replace silently, no flash (an agent
      // edit that caused this open is revealed by `scrollTarget`).
      readyRef.current = true;
      if (dirtyRef.current) {
        // Typed into the placeholder while loading: don't drop it, offer the disk text instead.
        if (cur !== next) setPendingExternal(next);
        return;
      }
      if (cur !== next) {
        view.dispatch({
          changes: { from: 0, to: view.state.doc.length, insert: next },
          annotations: externalChange.of(true),
        });
      }
      if (jumpToLine) jumpTo(view, jumpToLine, jumpToColumn, jumpToEndColumn);
      return;
    }
    if (cur === next) {
      setPendingExternal(null);
      return;
    }
    if (dirtyRef.current) {
      setPendingExternal(next);
      return;
    }
    const busy = !!userActivityRef && Date.now() - userActivityRef.current < 4000;
    if (applyExternalContent(view, next, !!followEdits && !busy)) {
      setLiveAt(Date.now());
      armFlashClear();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [content, binary, loading]);

  // Keep the inline review baseline in sync with the app's state (only once the real text is in:
  // diffing a baseline against the '' placeholder would paint the whole file as deleted).
  useEffect(() => {
    const view = viewRef.current;
    const field = reviewFieldRef.current;
    if (!view || !field || loading || !readyRef.current) return;
    const cur = view.state.field(field).base;
    const next = reviewBase == null ? null : reviewBase.replace(/\r\n?/g, '\n');
    if (cur !== next) view.dispatch({ effects: setReviewBase.of(next) });
  }, [reviewBase, path, theme, loading]);

  const handledScrollRef = useRef(0);
  useEffect(() => {
    const view = viewRef.current;
    if (!view || loading || !readyRef.current || !scrollTarget) return;
    if (handledScrollRef.current === scrollTarget.key) return;
    const doc = view.state.doc;
    // The edit's text is not in the view yet (fresh content still loading): wait for it.
    if (scrollTarget.line < 1 || scrollTarget.line > doc.lines) return;
    const t = scrollTarget.text;
    // Not showing the edited text yet (fresh read in flight) or never will (unsaved edits keep
    // it out): don't glow or scroll on lines that are not the ones the agent wrote.
    if (t !== undefined && (doc.length !== t.length || doc.toString() !== t)) return;
    handledScrollRef.current = scrollTarget.key;
    const busy =
      dirtyRef.current || (!!userActivityRef && Date.now() - userActivityRef.current < 4000);

    // Arrival glow for the lines just written, unless the in-place patch above already did it.
    const ranges = scrollTarget.ranges ?? [];
    if (ranges.length && !dirtyRef.current && (scrollTarget.reveal || view.state.field(flashField).size === 0)) {
      const starts: number[] = [];
      for (const r of ranges) {
        for (let l = Math.max(1, r.from); l <= Math.min(r.to, doc.lines) && starts.length < 500; l++) {
          starts.push(doc.line(l).from);
        }
      }
      if (starts.length) {
        view.dispatch({ effects: setFlash.of(starts) });
        armFlashClear();
      }
    }

    if (!scrollTarget.reveal && (!followEdits || busy)) return;
    const pos = doc.line(scrollTarget.line).from;
    const go = () => {
      if (viewRef.current !== view) return;
      view.dispatch({ effects: EditorView.scrollIntoView(pos, { y: 'center' }) });
    };
    if (isMarkdown && modeRef.current === 'preview') {
      // The editor is hidden behind the preview: show the source, then scroll once it is laid out.
      setMode('source');
      requestAnimationFrame(() => {
        view.requestMeasure();
        go();
      });
    } else {
      go();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scrollTarget, content, followEdits, loading]);

  /** Move the cursor to the next / previous agent change (wraps around). */
  navRef.current = (dir) => {
    const view = viewRef.current;
    const field = reviewFieldRef.current;
    if (!view || !field) return false;
    const rv = view.state.field(field);
    if (rv.base === null || rv.hunks.length === 0) return false;
    const lines = hunkLines(view.state, rv.hunks);
    const cur = view.state.doc.lineAt(view.state.selection.main.head).number;
    let idx: number;
    if (dir === 1) {
      idx = lines.findIndex((l) => l > cur);
      if (idx < 0) idx = 0;
    } else {
      idx = -1;
      for (let i = lines.length - 1; i >= 0; i--) {
        if (lines[i] < cur) {
          idx = i;
          break;
        }
      }
      if (idx < 0) idx = lines.length - 1;
    }
    goToHunk(idx);
    return true;
  };

  const goToHunk = (idx: number) => {
    const view = viewRef.current;
    const field = reviewFieldRef.current;
    if (!view || !field) return;
    const rv = view.state.field(field);
    const h = rv.hunks[idx];
    if (!h) return;
    const pos = view.state.doc.lineAt(Math.min(h.newFrom, view.state.doc.length)).from;
    view.dispatch({
      selection: { anchor: pos },
      effects: EditorView.scrollIntoView(pos, { y: 'center' }),
    });
    view.focus();
    setNavIdx(idx);
  };

  reviewHandlersRef.current = {
    keep: (view, h) => {
      const field = reviewFieldRef.current;
      const base = field ? view.state.field(field).base : null;
      if (base === null) return;
      const nb = base.slice(0, h.oldFrom) + h.added + base.slice(h.oldTo);
      view.dispatch({ effects: setReviewBase.of(nb) });
      onReviewBaseChangeRef.current?.(nb);
    },
    undo: (view, h) => {
      const wasDirty = dirtyRef.current;
      view.dispatch({
        changes: { from: h.newFrom, to: h.newTo, insert: h.removed },
        annotations: externalChange.of(true),
      });
      if (!wasDirty) void handleSave();
    },
  };

  const undoFile = () => {
    const view = viewRef.current;
    const field = reviewFieldRef.current;
    const base = view && field ? view.state.field(field).base : null;
    if (!view || base === null) return;
    const before = view.state.doc.toString();
    const wasDirty = dirtyRef.current;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: base },
      annotations: externalChange.of(true),
    });
    if (!wasDirty) void handleSave();
    onFileUndoneRef.current?.(before, base);
  };

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
      <EmptyState
        size="lg"
        icon={Binary}
        title="Binary file"
        description="This file can't be displayed in the text editor."
        className="flex-1"
      />
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
          {reviewCount > 0 && (
            <div className="flex items-center rounded border border-[var(--border)] overflow-hidden text-[10.5px] shrink-0">
              <button
                onClick={() => navRef.current(-1)}
                className="h-5 px-1 flex items-center hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer"
                title="Previous agent change (Alt+[)"
              >
                <ChevronUp size={11} />
              </button>
              <button
                onClick={() => navRef.current(1)}
                className="h-5 px-1 flex items-center border-l border-[var(--border)] hover:bg-[var(--bg-hover)] text-[var(--text-muted)] cursor-pointer"
                title="Next agent change (Alt+])"
              >
                <ChevronDown size={11} />
              </button>
              <span className="px-1.5 border-l border-[var(--border)] text-[var(--text-muted)] tabular-nums">
                {navIdx >= 0 && navIdx < reviewCount
                  ? `${navIdx + 1} / ${reviewCount}`
                  : `${reviewCount} agent change${reviewCount > 1 ? 's' : ''}`}
              </span>
              <button
                onClick={() => onReviewBaseChange?.(null)}
                className="px-1.5 h-5 flex items-center gap-1 border-l border-[var(--border)] hover:bg-[var(--bg-hover)] text-[var(--added-fg)] cursor-pointer"
                title="Keep all changes in this file"
              >
                <Check size={10.5} />
                Keep file
              </button>
              <button
                onClick={undoFile}
                className="px-1.5 h-5 flex items-center gap-1 border-l border-[var(--border)] hover:bg-[var(--bg-hover)] text-[var(--removed-fg)] cursor-pointer"
                title="Revert this file to before the agent's edits"
              >
                <RotateCcw size={10.5} />
                Undo file
              </button>
            </div>
          )}
          {liveAt > 0 && (
            <span
              key={liveAt}
              className="live-note flex items-center gap-1.5 text-[11px] text-[var(--text-dim)] shrink-0"
              title="This file was updated on disk and reloaded"
            >
              <span className="live-ping relative w-1.5 h-1.5 rounded-full bg-[var(--added-fg)]" />
              Synced from disk
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
            <Suspense fallback={null}>
              <MarkdownMessage content={previewText} theme={theme} variant="document" filePath={path} />
            </Suspense>
          </div>
        </div>
      )}
      <div
        className={`relative flex-1 min-h-0 ${isMarkdown && mode === 'preview' ? 'hidden' : 'flex flex-col'}`}
      >
        <div ref={containerRef} className="flex-1 min-h-0 overflow-hidden" />
        {reviewCount > 0 && reviewMarks.lines.length > 0 && (
          // Overview ruler: where the unreviewed agent changes are in the whole file.
          <div className="cm-review-ruler" aria-hidden="true">
            {reviewMarks.lines.map((m, i) => (
              <div
                key={i}
                className={`cm-review-tick cm-review-tick-${m.kind}`}
                style={{
                  top: `${((m.line - 1) / Math.max(1, reviewMarks.total)) * 100}%`,
                  height: `max(3px, ${(m.span / Math.max(1, reviewMarks.total)) * 100}%)`,
                }}
                onClick={() => goToHunk(i)}
                title={`Change ${i + 1} of ${reviewMarks.lines.length}`}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
};
