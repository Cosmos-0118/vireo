// Workbench layout constraints & solver.
//
// Panel widths the user picks ("preferred") are stored untouched; what is rendered ("effective")
// is derived from the measured workbench size, so shrinking the window and growing it back
// restores the user's layout instead of permanently destroying it (same idea as VS Code's
// min/max-constrained grid with layout priorities).

export const SIDEBAR = { min: 220, max: 560, def: 320 };
export const AGENT = { min: 280, max: 620, def: 400 };
export const TERMINAL = { min: 100, def: 240 };
/** The editor/diff column must never be squeezed below this. */
export const CENTER_MIN_WIDTH = 340;
export const EDITOR_MIN_HEIGHT = 140;
export const HANDLE = 4;
export const TAB_BAR_HEIGHT = 40;

export type PanelSide = 'sidebar' | 'agent';

export interface WidthSolution {
  sidebarVisible: boolean;
  agentVisible: boolean;
  sidebarWidth: number;
  agentWidth: number;
  /** Largest width the user can drag each panel to right now. */
  sidebarMax: number;
  agentMax: number;
}

const clamp = (n: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, n));

export function solveWidths(opts: {
  total: number; // measured workbench width; 0 = not measured yet
  wantSidebar: boolean;
  wantAgent: boolean;
  sidebar: number;
  agent: number;
  priority: PanelSide; // which panel survives when both cannot fit
}): WidthSolution {
  const { total, wantSidebar, wantAgent, priority } = opts;
  let sidebar = clamp(opts.sidebar, SIDEBAR.min, SIDEBAR.max);
  let agent = clamp(opts.agent, AGENT.min, AGENT.max);

  if (total <= 0) {
    return {
      sidebarVisible: wantSidebar,
      agentVisible: wantAgent,
      sidebarWidth: sidebar,
      agentWidth: agent,
      sidebarMax: SIDEBAR.max,
      agentMax: AGENT.max,
    };
  }

  let s = wantSidebar;
  let a = wantAgent;
  const budget = () => total - CENTER_MIN_WIDTH - (s ? HANDLE : 0) - (a ? HANDLE : 0);
  const minSum = () => (s ? SIDEBAR.min : 0) + (a ? AGENT.min : 0);

  // Not enough room even at minimum sizes: auto-collapse the lower-priority panel, then, if
  // still impossible (tiny window), the other one.
  if (s && a && minSum() > budget()) {
    if (priority === 'sidebar') a = false;
    else s = false;
  }
  if (s && minSum() > budget()) s = false;
  if (a && minSum() > budget()) a = false;

  // Shrink proportionally to each panel's slack above its minimum.
  const room = Math.max(0, budget());
  const sSlack = s ? sidebar - SIDEBAR.min : 0;
  const aSlack = a ? agent - AGENT.min : 0;
  const excess = (s ? sidebar : 0) + (a ? agent : 0) - room;
  if (excess > 0 && sSlack + aSlack > 0) {
    const cut = Math.min(excess, sSlack + aSlack);
    if (s) sidebar -= (cut * sSlack) / (sSlack + aSlack);
    if (a) agent -= (cut * aSlack) / (sSlack + aSlack);
  }
  sidebar = Math.round(sidebar);
  agent = Math.round(agent);

  return {
    sidebarVisible: s,
    agentVisible: a,
    sidebarWidth: sidebar,
    agentWidth: agent,
    sidebarMax: clamp(room - (a ? agent : 0), SIDEBAR.min, SIDEBAR.max),
    agentMax: clamp(room - (s ? sidebar : 0), AGENT.min, AGENT.max),
  };
}

export function solveTerminalHeight(opts: {
  totalHeight: number; // measured workbench height; 0 = not measured yet
  hasTabBar: boolean;
  preferred: number;
}): { height: number; max: number } {
  const { totalHeight, hasTabBar, preferred } = opts;
  if (totalHeight <= 0) return { height: Math.max(TERMINAL.min, preferred), max: 520 };
  const max = Math.max(
    TERMINAL.min,
    Math.min(totalHeight - EDITOR_MIN_HEIGHT - HANDLE - (hasTabBar ? TAB_BAR_HEIGHT : 0), 900)
  );
  return { height: clamp(preferred, TERMINAL.min, max), max };
}

const KEY = 'vireo-layout-v1';

export interface StoredLayout {
  sidebarWidth: number;
  agentWidth: number;
  terminalHeight: number;
}

export function loadLayout(): StoredLayout {
  const fallback = { sidebarWidth: SIDEBAR.def, agentWidth: AGENT.def, terminalHeight: TERMINAL.def };
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) || 'null');
    if (!raw || typeof raw !== 'object') return fallback;
    const num = (v: unknown, d: number) => (typeof v === 'number' && Number.isFinite(v) ? v : d);
    return {
      sidebarWidth: num(raw.sidebarWidth, fallback.sidebarWidth),
      agentWidth: num(raw.agentWidth, fallback.agentWidth),
      terminalHeight: num(raw.terminalHeight, fallback.terminalHeight),
    };
  } catch {
    return fallback;
  }
}

export function saveLayout(layout: StoredLayout) {
  try {
    localStorage.setItem(KEY, JSON.stringify(layout));
  } catch {
    // ignore
  }
}
