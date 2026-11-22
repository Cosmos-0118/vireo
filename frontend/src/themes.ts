export interface LiteTheme {
  id: string;
  name: string;
  type: 'dark' | 'light';
  vars: {
    '--bg-base': string;
    '--bg-sidebar': string;
    '--bg-activity': string;
    '--bg-panel': string;
    '--bg-elevated': string;
    '--bg-hover': string;
    '--bg-active': string;
    '--bg-input': string;
    '--border': string;
    '--text-primary': string;
    '--text-muted': string;
    '--text-dim': string;
    '--accent': string;
    '--accent-hover': string;
    '--accent-fg': string;
    '--added-bg': string;
    '--added-word': string;
    '--added-fg': string;
    '--removed-bg': string;
    '--removed-word': string;
    '--removed-fg': string;
    '--modified-fg': string;
    '--danger': string;
    '--status-bar-bg': string;
    '--status-bar-fg': string;
  };
  syntax: {
    keyword: string;
    string: string;
    number: string;
    comment: string;
    func: string;
    type: string;
    variable: string;
    operator: string;
    /** Member access / object keys; falls back to `variable` when unset. */
    property?: string;
    /** Decorators, attributes, markup markers; falls back to `type` when unset. */
    meta?: string;
  };
  terminal: {
    background: string;
    foreground: string;
    cursor: string;
    selectionBackground: string;
    black: string;
    red: string;
    green: string;
    yellow: string;
    blue: string;
    magenta: string;
    cyan: string;
    white: string;
  };
}

export const BUILTIN_THEMES: LiteTheme[] = [
  {
    id: 'vscode-dark-plus',
    name: 'VS Code Dark+',
    type: 'dark',
    vars: {
      '--bg-base': '#1e1f22',
      '--bg-sidebar': '#18191c',
      '--bg-activity': '#131417',
      '--bg-panel': '#16171a',
      '--bg-elevated': '#26282e',
      '--bg-hover': '#2a2d35',
      '--bg-active': '#323743',
      '--bg-input': '#141518',
      '--border': '#2e3139',
      '--text-primary': '#e4e7ee',
      '--text-muted': '#9da5b4',
      '--text-dim': '#727a88',
      '--accent': '#2f81f7',
      '--accent-hover': '#4b93f8',
      '--accent-fg': '#ffffff',
      '--added-bg': 'rgba(46, 160, 67, 0.15)',
      '--added-word': 'rgba(46, 160, 67, 0.38)',
      '--added-fg': '#4ade80',
      '--removed-bg': 'rgba(248, 81, 73, 0.15)',
      '--removed-word': 'rgba(248, 81, 73, 0.38)',
      '--removed-fg': '#f87171',
      '--modified-fg': '#fbbf24',
      '--danger': '#ef4444',
      '--status-bar-bg': '#131417',
      '--status-bar-fg': '#cbd5e1',
    },
    syntax: {
      keyword: '#569cd6',
      string: '#ce9178',
      number: '#b5cea8',
      comment: '#75a862',
      func: '#dcdcaa',
      type: '#4ec9b0',
      variable: '#9cdcfe',
      property: '#9cdcfe',
      meta: '#c586c0',
      operator: '#d4d4d4',
    },
    terminal: {
      background: '#16171a',
      foreground: '#dcdfe6',
      cursor: '#aeafad',
      selectionBackground: '#264f78',
      black: '#111215',
      red: '#f14c4c',
      green: '#23d18b',
      yellow: '#f5f543',
      blue: '#3b8eea',
      magenta: '#d670d6',
      cyan: '#29b8db',
      white: '#e5e5e5',
    },
  },
  {
    id: 'tokyo-night',
    name: 'Tokyo Night',
    type: 'dark',
    vars: {
      '--bg-base': '#1a1b26',
      '--bg-sidebar': '#15161f',
      '--bg-activity': '#11121a',
      '--bg-panel': '#15161f',
      '--bg-elevated': '#23273a',
      '--bg-hover': '#252a3f',
      '--bg-active': '#2f354f',
      '--bg-input': '#13141d',
      '--border': '#2b3048',
      '--text-primary': '#d5dcf8',
      '--text-muted': '#9aa5ce',
      '--text-dim': '#6e78a4',
      '--accent': '#7aa2f7',
      '--accent-hover': '#8eb0fb',
      '--accent-fg': '#11121a',
      '--added-bg': 'rgba(158, 206, 106, 0.14)',
      '--added-word': 'rgba(158, 206, 106, 0.34)',
      '--added-fg': '#9ece6a',
      '--removed-bg': 'rgba(247, 118, 142, 0.15)',
      '--removed-word': 'rgba(247, 118, 142, 0.36)',
      '--removed-fg': '#f7768e',
      '--modified-fg': '#e0af68',
      '--danger': '#f7768e',
      '--status-bar-bg': '#11121a',
      '--status-bar-fg': '#a9b1d6',
    },
    syntax: {
      keyword: '#bb9af7',
      string: '#9ece6a',
      number: '#ff9e64',
      comment: '#6b75a2',
      func: '#7aa2f7',
      type: '#2ac3de',
      variable: '#c0caf5',
      property: '#73daca',
      meta: '#e0af68',
      operator: '#89ddff',
    },
    terminal: {
      background: '#15161f',
      foreground: '#d5dcf8',
      cursor: '#c0caf5',
      selectionBackground: '#33467c',
      black: '#15161e',
      red: '#f7768e',
      green: '#9ece6a',
      yellow: '#e0af68',
      blue: '#7aa2f7',
      magenta: '#bb9af7',
      cyan: '#7dcfff',
      white: '#a9b1d6',
    },
  },
  {
    id: 'catppuccin-mocha',
    name: 'Catppuccin Mocha',
    type: 'dark',
    vars: {
      '--bg-base': '#1e1e2e',
      '--bg-sidebar': '#181825',
      '--bg-activity': '#11111b',
      '--bg-panel': '#181825',
      '--bg-elevated': '#2a2b3d',
      '--bg-hover': '#2e3044',
      '--bg-active': '#383a52',
      '--bg-input': '#141421',
      '--border': '#33354a',
      '--text-primary': '#dce2fa',
      '--text-muted': '#a6adc8',
      '--text-dim': '#7c819a',
      '--accent': '#cba6f7',
      '--accent-hover': '#d7b9fa',
      '--accent-fg': '#11111b',
      '--added-bg': 'rgba(166, 227, 161, 0.14)',
      '--added-word': 'rgba(166, 227, 161, 0.34)',
      '--added-fg': '#a6e3a1',
      '--removed-bg': 'rgba(243, 139, 168, 0.15)',
      '--removed-word': 'rgba(243, 139, 168, 0.36)',
      '--removed-fg': '#f38ba8',
      '--modified-fg': '#f9e2af',
      '--danger': '#f38ba8',
      '--status-bar-bg': '#11111b',
      '--status-bar-fg': '#cdd6f4',
    },
    syntax: {
      keyword: '#cba6f7',
      string: '#a6e3a1',
      number: '#fab387',
      comment: '#7c819a',
      func: '#89b4fa',
      type: '#f9e2af',
      variable: '#cdd6f4',
      property: '#94e2d5',
      meta: '#f5c2e7',
      operator: '#89dceb',
    },
    terminal: {
      background: '#181825',
      foreground: '#dce2fa',
      cursor: '#f5e0dc',
      selectionBackground: '#45475a',
      black: '#45475a',
      red: '#f38ba8',
      green: '#a6e3a1',
      yellow: '#f9e2af',
      blue: '#89b4fa',
      magenta: '#f5c2e7',
      cyan: '#94e2d5',
      white: '#bac2de',
    },
  },
  {
    id: 'one-dark-pro',
    name: 'One Dark Pro',
    type: 'dark',
    vars: {
      '--bg-base': '#23272e',
      '--bg-sidebar': '#1d2026',
      '--bg-activity': '#181a1f',
      '--bg-panel': '#1d2026',
      '--bg-elevated': '#2b303a',
      '--bg-hover': '#2e3440',
      '--bg-active': '#383f4d',
      '--bg-input': '#191c22',
      '--border': '#343a46',
      '--text-primary': '#cfd6e2',
      '--text-muted': '#939ba8',
      '--text-dim': '#717988',
      '--accent': '#61afef',
      '--accent-hover': '#78bcf2',
      '--accent-fg': '#181a1f',
      '--added-bg': 'rgba(152, 195, 121, 0.14)',
      '--added-word': 'rgba(152, 195, 121, 0.35)',
      '--added-fg': '#98c379',
      '--removed-bg': 'rgba(224, 108, 117, 0.15)',
      '--removed-word': 'rgba(224, 108, 117, 0.36)',
      '--removed-fg': '#e06c75',
      '--modified-fg': '#e5c07b',
      '--danger': '#e06c75',
      '--status-bar-bg': '#181a1f',
      '--status-bar-fg': '#b8c0cc',
    },
    syntax: {
      keyword: '#c678dd',
      string: '#98c379',
      number: '#d19a66',
      comment: '#757e8e',
      func: '#61afef',
      type: '#e5c07b',
      variable: '#e06c75',
      property: '#d19a66',
      meta: '#e5c07b',
      operator: '#56b6c2',
    },
    terminal: {
      background: '#1d2026',
      foreground: '#cfd6e2',
      cursor: '#528bff',
      selectionBackground: '#3e4451',
      black: '#23272e',
      red: '#e06c75',
      green: '#98c379',
      yellow: '#e5c07b',
      blue: '#61afef',
      magenta: '#c678dd',
      cyan: '#56b6c2',
      white: '#dcdfe4',
    },
  },
  {
    id: 'github-dark-dimmed',
    name: 'GitHub Dark Dimmed',
    type: 'dark',
    vars: {
      '--bg-base': '#22272e',
      '--bg-sidebar': '#1c2128',
      '--bg-activity': '#161b22',
      '--bg-panel': '#1c2128',
      '--bg-elevated': '#2b3139',
      '--bg-hover': '#2d333b',
      '--bg-active': '#373e47',
      '--bg-input': '#191e24',
      '--border': '#373e47',
      '--text-primary': '#cdd9e5',
      '--text-muted': '#8b949e',
      '--text-dim': '#6c7682',
      '--accent': '#539bf5',
      '--accent-hover': '#6cb6ff',
      '--accent-fg': '#ffffff',
      '--added-bg': 'rgba(87, 171, 90, 0.16)',
      '--added-word': 'rgba(87, 171, 90, 0.38)',
      '--added-fg': '#57ab5a',
      '--removed-bg': 'rgba(229, 83, 75, 0.16)',
      '--removed-word': 'rgba(229, 83, 75, 0.38)',
      '--removed-fg': '#f47067',
      '--modified-fg': '#daaa3f',
      '--danger': '#e5534b',
      '--status-bar-bg': '#161b22',
      '--status-bar-fg': '#adbac7',
    },
    syntax: {
      keyword: '#f47067',
      string: '#96d0ff',
      number: '#6cb6ff',
      comment: '#7e8b99',
      func: '#dcbdfb',
      type: '#f69d50',
      variable: '#cdd9e5',
      property: '#6cb6ff',
      meta: '#daaa3f',
      operator: '#f47067',
    },
    terminal: {
      background: '#1c2128',
      foreground: '#cdd9e5',
      cursor: '#539bf5',
      selectionBackground: '#316dca55',
      black: '#545d68',
      red: '#f47067',
      green: '#57ab5a',
      yellow: '#daaa3f',
      blue: '#539bf5',
      magenta: '#b083f0',
      cyan: '#39c5cf',
      white: '#909dab',
    },
  },
  {
    id: 'github-light',
    name: 'GitHub Light',
    type: 'light',
    vars: {
      '--bg-base': '#ffffff',
      '--bg-sidebar': '#f6f8fa',
      '--bg-activity': '#eef1f5',
      '--bg-panel': '#f6f8fa',
      '--bg-elevated': '#ffffff',
      '--bg-hover': '#eaeef2',
      '--bg-active': '#dde3ea',
      '--bg-input': '#ffffff',
      '--border': '#d0d7de',
      '--text-primary': '#1f2328',
      '--text-muted': '#57606a',
      '--text-dim': '#76808b',
      '--accent': '#0969da',
      '--accent-hover': '#0550ae',
      '--accent-fg': '#ffffff',
      '--added-bg': 'rgba(45, 164, 78, 0.13)',
      '--added-word': 'rgba(45, 164, 78, 0.32)',
      '--added-fg': '#1a7f37',
      '--removed-bg': 'rgba(207, 34, 46, 0.12)',
      '--removed-word': 'rgba(207, 34, 46, 0.30)',
      '--removed-fg': '#cf222e',
      '--modified-fg': '#9a6700',
      '--danger': '#cf222e',
      '--status-bar-bg': '#eef1f5',
      '--status-bar-fg': '#1f2328',
    },
    syntax: {
      keyword: '#cf222e',
      string: '#0a3069',
      number: '#0550ae',
      comment: '#6e7781',
      func: '#8250df',
      type: '#953800',
      variable: '#1f2328',
      property: '#0550ae',
      meta: '#116329',
      operator: '#cf222e',
    },
    terminal: {
      background: '#f6f8fa',
      foreground: '#1f2328',
      cursor: '#0969da',
      selectionBackground: '#b6e3ff',
      black: '#24292f',
      red: '#cf222e',
      green: '#116329',
      yellow: '#4d2d00',
      blue: '#0969da',
      magenta: '#8250df',
      cyan: '#1b7c83',
      white: '#6e7781',
    },
  },
];

export function applyTheme(theme: LiteTheme) {
  const root = document.documentElement;
  root.dataset.themeType = theme.type;
  root.style.colorScheme = theme.type;
  for (const [k, v] of Object.entries(theme.vars)) {
    root.style.setProperty(k, v);
  }
}

/**
 * Parse a standard VS Code theme JSON file into a LiteTheme so users can
 * drop in any VS Code color theme directly.
 */
export function parseVsCodeThemeJson(raw: string, fallbackName = 'Imported VS Code Theme'): LiteTheme {
  // Strip single-line // comments common in VS Code JSONC themes
  const stripped = raw.replace(/^\s*\/\/.*$/gm, '').replace(/,\s*([}\]])/g, '$1');
  const parsed = JSON.parse(stripped) as {
    name?: string;
    type?: string;
    colors?: Record<string, string>;
    tokenColors?: Array<{ scope?: string | string[]; settings?: { foreground?: string } }>;
  };

  const isLight = parsed.type === 'light';
  const base = isLight ? BUILTIN_THEMES[5] : BUILTIN_THEMES[0];
  const c = parsed.colors ?? {};

  const pickToken = (needle: string, fallback: string) => {
    for (const t of parsed.tokenColors ?? []) {
      const scopes = Array.isArray(t.scope) ? t.scope : t.scope ? t.scope.split(',') : [];
      if (scopes.some((s) => s.toLowerCase().includes(needle)) && t.settings?.foreground) {
        return t.settings.foreground;
      }
    }
    return fallback;
  };

  const name = parsed.name || fallbackName;
  const bg = c['editor.background'] || base.vars['--bg-base'];
  const fg = c['editor.foreground'] || base.vars['--text-primary'];
  const sidebar = c['sideBar.background'] || bg;
  const panel = c['panel.background'] || sidebar;
  const accent = c['button.background'] || c['focusBorder'] || base.vars['--accent'];

  return {
    id: `custom-${name.toLowerCase().replace(/[^a-z0-9]+/g, '-')}`,
    name,
    type: isLight ? 'light' : 'dark',
    vars: {
      ...base.vars,
      '--bg-base': bg,
      '--bg-sidebar': sidebar,
      '--bg-activity': c['activityBar.background'] || sidebar,
      '--bg-panel': panel,
      '--bg-elevated': c['editorWidget.background'] || c['dropdown.background'] || base.vars['--bg-elevated'],
      '--bg-hover': c['list.hoverBackground'] || base.vars['--bg-hover'],
      '--bg-active': c['list.activeSelectionBackground'] || base.vars['--bg-active'],
      '--bg-input': c['input.background'] || bg,
      '--border': c['panel.border'] || c['sideBar.border'] || base.vars['--border'],
      '--text-primary': fg,
      '--text-muted': c['sideBar.foreground'] || base.vars['--text-muted'],
      '--text-dim': c['editorLineNumber.foreground'] || base.vars['--text-dim'],
      '--accent': accent,
      '--accent-hover': c['button.hoverBackground'] || accent,
      '--accent-fg': c['button.foreground'] || base.vars['--accent-fg'],
      '--status-bar-bg': c['statusBar.background'] || accent,
      '--status-bar-fg': c['statusBar.foreground'] || '#ffffff',
    },
    syntax: {
      keyword: pickToken('keyword', base.syntax.keyword),
      string: pickToken('string', base.syntax.string),
      number: pickToken('numeric', base.syntax.number),
      comment: pickToken('comment', base.syntax.comment),
      func: pickToken('function', base.syntax.func),
      type: pickToken('type', base.syntax.type),
      variable: pickToken('variable', base.syntax.variable),
      operator: pickToken('operator', base.syntax.operator),
      property: pickToken('property', base.syntax.property ?? base.syntax.variable),
      meta: pickToken('decorator', base.syntax.meta ?? base.syntax.type),
    },
    terminal: {
      ...base.terminal,
      background: c['terminal.background'] || panel,
      foreground: c['terminal.foreground'] || fg,
      black: c['terminal.ansiBlack'] || base.terminal.black,
      red: c['terminal.ansiRed'] || base.terminal.red,
      green: c['terminal.ansiGreen'] || base.terminal.green,
      yellow: c['terminal.ansiYellow'] || base.terminal.yellow,
      blue: c['terminal.ansiBlue'] || base.terminal.blue,
      magenta: c['terminal.ansiMagenta'] || base.terminal.magenta,
      cyan: c['terminal.ansiCyan'] || base.terminal.cyan,
      white: c['terminal.ansiWhite'] || base.terminal.white,
    },
  };
}
