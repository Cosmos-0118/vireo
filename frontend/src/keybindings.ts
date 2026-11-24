// Keyboard shortcut registry: defaults, user overrides (localStorage), matching and formatting.
//
// A binding is a string like "Mod+Shift+F", "Mod+`" or "F5". `Mod` means Ctrl (or Cmd on macOS).
// Modifiers are always written in the order Mod, Alt, Shift; the key token comes last.
// An empty string means "unbound".

import { useSyncExternalStore } from 'react';

export type ActionId =
  | 'quickOpen'
  | 'openSettings'
  | 'openShortcuts'
  | 'saveFile'
  | 'runPrimary'
  | 'toggleSidebar'
  | 'toggleTerminal'
  | 'toggleAgent'
  | 'showExplorer'
  | 'showSearch'
  | 'showSourceControl'
  | 'openGitGraph'
  | 'closeTab'
  | 'closeAllTabs'
  | 'nextTab'
  | 'prevTab'
  | 'zoomIn'
  | 'zoomOut'
  | 'zoomReset';

export type ShortcutGroup = 'General' | 'View' | 'Sidebar' | 'Tabs' | 'Zoom';

export interface ActionDef {
  id: ActionId;
  label: string;
  group: ShortcutGroup;
  /** Default binding. */
  default: string;
  /** Extra built-in bindings that always work and cannot be edited (layout / numpad variants). */
  alts?: string[];
}

export const GROUP_ORDER: ShortcutGroup[] = ['General', 'View', 'Sidebar', 'Tabs', 'Zoom'];

export const ACTIONS: ActionDef[] = [
  // General
  { id: 'quickOpen', label: 'Quick Open (files, themes, tasks)', group: 'General', default: 'Mod+P' },
  { id: 'openSettings', label: 'Open Settings', group: 'General', default: 'Mod+,' },
  { id: 'openShortcuts', label: 'Open Keyboard Shortcuts', group: 'General', default: 'Mod+Shift+/' },
  { id: 'saveFile', label: 'Save File', group: 'General', default: 'Mod+S', alts: ['Mod+Shift+S'] },
  { id: 'runPrimary', label: 'Run primary task (restarts if running)', group: 'General', default: 'F5' },
  // View
  { id: 'toggleSidebar', label: 'Toggle Sidebar', group: 'View', default: 'Mod+B' },
  { id: 'toggleTerminal', label: 'Toggle Terminal', group: 'View', default: 'Mod+`' },
  { id: 'toggleAgent', label: 'Toggle Agent Panel', group: 'View', default: 'Mod+Shift+A' },
  // Sidebar views
  { id: 'showExplorer', label: 'Show File Explorer', group: 'Sidebar', default: 'Mod+Shift+E' },
  { id: 'showSearch', label: 'Search in Files', group: 'Sidebar', default: 'Mod+Shift+F' },
  { id: 'showSourceControl', label: 'Show Source Control', group: 'Sidebar', default: 'Mod+Shift+G' },
  { id: 'openGitGraph', label: 'Open Git Graph', group: 'Sidebar', default: 'Mod+Alt+G' },
  // Tabs
  { id: 'closeTab', label: 'Close Active Tab', group: 'Tabs', default: 'Mod+W' },
  { id: 'closeAllTabs', label: 'Close All Tabs', group: 'Tabs', default: 'Mod+Shift+W' },
  { id: 'nextTab', label: 'Next Tab', group: 'Tabs', default: 'Mod+Tab', alts: ['Mod+PageDown'] },
  { id: 'prevTab', label: 'Previous Tab', group: 'Tabs', default: 'Mod+Shift+Tab', alts: ['Mod+PageUp'] },
  // Zoom
  { id: 'zoomIn', label: 'Zoom In', group: 'Zoom', default: 'Mod+=', alts: ['Mod+Shift+=', 'Mod+NumAdd'] },
  { id: 'zoomOut', label: 'Zoom Out', group: 'Zoom', default: 'Mod+-', alts: ['Mod+Shift+-', 'Mod+NumSub'] },
  { id: 'zoomReset', label: 'Reset Zoom', group: 'Zoom', default: 'Mod+0', alts: ['Mod+Num0'] },
];

const ACTION_BY_ID = new Map(ACTIONS.map((a) => [a.id, a]));

export interface FixedShortcut {
  keys: string;
  label: string;
}

/** Shortcuts owned by the editor, terminal and dialogs. Listed for reference, not editable. */
export const FIXED_SHORTCUTS: { group: string; items: FixedShortcut[] }[] = [
  {
    group: 'Editor',
    items: [
      { keys: 'Alt+[', label: 'Navigate back' },
      { keys: 'Alt+]', label: 'Navigate forward' },
      { keys: 'Mod+Z', label: 'Undo' },
      { keys: 'Mod+Y', label: 'Redo' },
      { keys: 'Mod+Shift+Z', label: 'Redo (alternate)' },
      { keys: 'Mod+A', label: 'Select all' },
      { keys: 'Mod+/', label: 'Toggle line comment' },
      { keys: 'Alt+Up', label: 'Move line up' },
      { keys: 'Alt+Down', label: 'Move line down' },
      { keys: 'Alt+Shift+Up', label: 'Copy line up' },
      { keys: 'Alt+Shift+Down', label: 'Copy line down' },
      { keys: 'Mod+Shift+K', label: 'Delete line' },
      { keys: 'Mod+Enter', label: 'Insert blank line below' },
      { keys: 'Mod+L', label: 'Select line' },
      { keys: 'Mod+]', label: 'Indent' },
      { keys: 'Mod+[', label: 'Outdent' },
      { keys: 'Mod+Shift+\\', label: 'Jump to matching bracket' },
    ],
  },
  {
    group: 'Terminal',
    items: [
      { keys: 'Mod+C', label: 'Copy selection (sends interrupt when nothing is selected)' },
      { keys: 'Mod+V', label: 'Paste' },
      { keys: 'Shift+Enter', label: 'Insert newline (for CLI agents)' },
    ],
  },
  {
    group: 'Dialogs & menus',
    items: [
      { keys: 'Esc', label: 'Close menu or dialog' },
      { keys: 'Enter', label: 'Confirm / run' },
      { keys: 'Mod+Enter', label: 'Commit (in the commit message box)' },
    ],
  },
];

// ---------------------------------------------------------------------------
// Platform
// ---------------------------------------------------------------------------

export const IS_MAC = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/i.test(navigator.platform || '');

// ---------------------------------------------------------------------------
// Binding <-> keyboard event
// ---------------------------------------------------------------------------

const CODE_TOKENS: Record<string, string> = {
  Equal: '=',
  Minus: '-',
  Backquote: '`',
  Comma: ',',
  Period: '.',
  Slash: '/',
  Backslash: '\\',
  BracketLeft: '[',
  BracketRight: ']',
  Semicolon: ';',
  Quote: "'",
  Space: 'Space',
  Tab: 'Tab',
  Enter: 'Enter',
  NumpadEnter: 'Enter',
  Escape: 'Esc',
  Backspace: 'Backspace',
  Delete: 'Delete',
  Insert: 'Insert',
  Home: 'Home',
  End: 'End',
  PageUp: 'PageUp',
  PageDown: 'PageDown',
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
  NumpadAdd: 'NumAdd',
  NumpadSubtract: 'NumSub',
};

function keyToken(e: KeyboardEvent): string | null {
  const c = e.code || '';
  if (/^Key[A-Z]$/.test(c)) return c.slice(3);
  if (/^Digit\d$/.test(c)) return c.slice(5);
  if (/^Numpad\d$/.test(c)) return `Num${c.slice(6)}`;
  if (/^F\d{1,2}$/.test(c)) return c;
  if (CODE_TOKENS[c]) return CODE_TOKENS[c];
  // No usable `code` (synthetic events, exotic layouts): fall back to `key`.
  const k = e.key;
  if (!k || k === 'Dead' || k === 'Unidentified') return null;
  if (k.length === 1) return k === ' ' ? 'Space' : k.toUpperCase();
  return k;
}

const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta', 'AltGraph', 'OS']);

/** Canonical binding string for a keydown event, or null while only modifiers are held. */
export function eventToBinding(e: KeyboardEvent): string | null {
  if (MODIFIER_KEYS.has(e.key)) return null;
  const tok = keyToken(e);
  if (!tok) return null;
  const parts: string[] = [];
  if (e.ctrlKey || e.metaKey) parts.push('Mod');
  if (e.altKey) parts.push('Alt');
  if (e.shiftKey) parts.push('Shift');
  parts.push(tok);
  return parts.join('+');
}

/** A binding must not hijack plain typing: require Mod or Alt, or be a function key. */
export function isValidBinding(binding: string): boolean {
  if (!binding) return false;
  return binding.startsWith('Mod+') || binding.startsWith('Alt+') || /^(Shift\+)?F\d{1,2}$/.test(binding);
}

const KEY_LABELS_MAC: Record<string, string> = {
  Mod: '⌘',
  Alt: '⌥',
  Shift: '⇧',
  Up: '↑',
  Down: '↓',
  Left: '←',
  Right: '→',
  Enter: '↵',
  Backspace: '⌫',
  Delete: '⌦',
};

const KEY_LABELS: Record<string, string> = {
  Mod: 'Ctrl',
  Up: '↑',
  Down: '↓',
  Left: '←',
  Right: '→',
  NumAdd: 'Num +',
  NumSub: 'Num -',
};

/** Split a binding into display keys, e.g. "Mod+Shift+F" -> ["Ctrl", "Shift", "F"]. */
export function bindingKeys(binding: string): string[] {
  if (!binding) return [];
  // Split on "+" but keep a literal "+" key (written as "Mod++").
  const parts = binding.split(/\+(?=.)/);
  return parts.map((p) => {
    if (IS_MAC) return KEY_LABELS_MAC[p] ?? KEY_LABELS[p]?.replace('Ctrl', '⌃') ?? p;
    if (p.startsWith('Num') && /^Num\d$/.test(p)) return `Num ${p.slice(3)}`;
    return KEY_LABELS[p] ?? p;
  });
}

export function formatBinding(binding: string): string {
  const keys = bindingKeys(binding);
  if (keys.length === 0) return '';
  return IS_MAC ? keys.join('') : keys.join('+');
}

// ---------------------------------------------------------------------------
// Store (user overrides)
// ---------------------------------------------------------------------------

const STORAGE_KEY = 'vireo-keybindings-v1';

type Overrides = Partial<Record<ActionId, string>>;

function loadOverrides(): Overrides {
  try {
    const raw = JSON.parse(localStorage.getItem(STORAGE_KEY) || 'null');
    if (!raw || typeof raw !== 'object') return {};
    const out: Overrides = {};
    for (const [id, v] of Object.entries(raw)) {
      if (ACTION_BY_ID.has(id as ActionId) && typeof v === 'string' && (v === '' || isValidBinding(v))) {
        out[id as ActionId] = v;
      }
    }
    return out;
  } catch {
    return {};
  }
}

let overrides: Overrides = loadOverrides();
const listeners = new Set<() => void>();

function commit(next: Overrides) {
  overrides = next;
  try {
    if (Object.keys(next).length === 0) localStorage.removeItem(STORAGE_KEY);
    else localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // ignore (private mode / quota)
  }
  listeners.forEach((l) => l());
}

function subscribe(l: () => void) {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function getBinding(id: ActionId): string {
  const o = overrides[id];
  return o !== undefined ? o : ACTION_BY_ID.get(id)!.default;
}

export function isCustomized(id: ActionId): boolean {
  return overrides[id] !== undefined;
}

/** Set a binding ("" unbinds). Setting it back to the default drops the override. */
export function setBinding(id: ActionId, binding: string) {
  const next = { ...overrides };
  if (binding === ACTION_BY_ID.get(id)!.default) delete next[id];
  else next[id] = binding;
  commit(next);
}

export function resetBinding(id: ActionId) {
  if (overrides[id] === undefined) return;
  const next = { ...overrides };
  delete next[id];
  commit(next);
}

export function resetAllBindings() {
  if (Object.keys(overrides).length === 0) return;
  commit({});
}

/** Re-renders the caller whenever any binding changes. Returns a stable snapshot object. */
export function useKeybindings(): Overrides {
  return useSyncExternalStore(subscribe, () => overrides);
}

/** Display string for an action's current binding (empty when unbound). */
export function shortcutLabel(id: ActionId): string {
  return formatBinding(getBinding(id));
}

/** `"Quick Open (Ctrl+P)"`-style tooltip text. */
export function withShortcut(text: string, id: ActionId): string {
  const s = shortcutLabel(id);
  return s ? `${text} (${s})` : text;
}

/** Other actions already using `binding` (own default/alts included). */
export function findConflicts(id: ActionId, binding: string): ActionDef[] {
  if (!binding) return [];
  return ACTIONS.filter(
    (a) => a.id !== id && (getBinding(a.id) === binding || a.alts?.includes(binding))
  );
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

let recording = 0;

/** While a shortcut is being recorded, global dispatch is suspended. Returns a release function. */
export function suspendShortcuts(): () => void {
  recording++;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    recording--;
  };
}

/** The action a keydown event triggers, if any. */
export function findAction(e: KeyboardEvent): ActionDef | undefined {
  if (recording > 0 || e.isComposing) return undefined;
  const b = eventToBinding(e);
  if (!b) return undefined;
  return ACTIONS.find((a) => getBinding(a.id) === b || a.alts?.includes(b));
}

export function matchesAction(e: KeyboardEvent, id: ActionId): boolean {
  return findAction(e)?.id === id;
}
