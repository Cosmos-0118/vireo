import React from 'react';
import {
  Atom,
  Braces,
  Code,
  Container,
  File,
  FileText,
  Folder,
  FolderCheck,
  FolderCode,
  FolderCog,
  FolderGit,
  FolderOpen,
  FolderTree,
  Hash,
  Image as ImageIcon,
  KeyRound,
  Lock,
  Package,
  Server,
  SlidersHorizontal,
  Terminal,
  Type,
  Zap,
  GitBranch,
  type LucideIcon,
} from 'lucide-react';

type IconSpec =
  | { kind: 'badge'; label: string; color: string }
  | { kind: 'lucide'; Icon: LucideIcon; color: string };

const badge = (label: string, color: string): IconSpec => ({ kind: 'badge', label, color });
const lucide = (Icon: LucideIcon, color: string): IconSpec => ({ kind: 'lucide', Icon, color });

const DIM = 'var(--text-dim)';
const SLATE = '#94a3b8';

const BY_NAME: Record<string, IconSpec> = {
  'package.json': lucide(Package, '#22c55e'),
  'cargo.toml': badge('Rs', '#f97316'),
  'cargo.lock': lucide(Lock, DIM),
  'package-lock.json': lucide(Lock, DIM),
  'pnpm-lock.yaml': lucide(Lock, DIM),
  'yarn.lock': lucide(Lock, DIM),
  'bun.lockb': lucide(Lock, DIM),
  'pyproject.toml': badge('Py', '#3b82f6'),
  'dockerfile': lucide(Container, '#38bdf8'),
  'docker-compose.yml': lucide(Container, '#38bdf8'),
  'docker-compose.yaml': lucide(Container, '#38bdf8'),
  '.gitignore': lucide(GitBranch, '#f97316'),
  '.gitattributes': lucide(GitBranch, '#f97316'),
  '.gitmodules': lucide(GitBranch, '#f97316'),
  'license': lucide(FileText, '#eab308'),
  'license.md': lucide(FileText, '#eab308'),
  'readme.md': lucide(FileText, '#60a5fa'),
  'makefile': lucide(Terminal, '#10b981'),
};

const BY_EXT: Record<string, IconSpec> = {
  ts: badge('TS', '#3178c6'),
  mts: badge('TS', '#3178c6'),
  cts: badge('TS', '#3178c6'),
  tsx: lucide(Atom, '#38bdf8'),
  js: badge('JS', '#facc15'),
  mjs: badge('JS', '#facc15'),
  cjs: badge('JS', '#facc15'),
  jsx: lucide(Atom, '#facc15'),
  rs: badge('Rs', '#f97316'),
  py: badge('Py', '#3b82f6'),
  go: badge('Go', '#06b6d4'),
  c: badge('C', '#60a5fa'),
  h: badge('H', '#60a5fa'),
  cpp: badge('C+', '#60a5fa'),
  hpp: badge('H+', '#60a5fa'),
  cc: badge('C+', '#60a5fa'),
  java: badge('Jv', '#ef4444'),
  kt: badge('Kt', '#a855f7'),
  swift: badge('Sw', '#f97316'),
  rb: badge('Rb', '#ef4444'),
  php: badge('Ph', '#8b5cf6'),
  html: lucide(Code, '#f97316'),
  htm: lucide(Code, '#f97316'),
  css: lucide(Hash, '#a855f7'),
  scss: lucide(Hash, '#ec4899'),
  sass: lucide(Hash, '#ec4899'),
  less: lucide(Hash, '#a855f7'),
  svg: lucide(ImageIcon, '#eab308'),
  json: lucide(Braces, '#eab308'),
  jsonc: lucide(Braces, '#eab308'),
  json5: lucide(Braces, '#eab308'),
  toml: lucide(SlidersHorizontal, SLATE),
  yaml: lucide(SlidersHorizontal, SLATE),
  yml: lucide(SlidersHorizontal, SLATE),
  ini: lucide(SlidersHorizontal, SLATE),
  cfg: lucide(SlidersHorizontal, SLATE),
  conf: lucide(SlidersHorizontal, SLATE),
  md: lucide(FileText, '#60a5fa'),
  mdx: lucide(FileText, '#60a5fa'),
  txt: lucide(FileText, SLATE),
  sh: lucide(Terminal, '#10b981'),
  bash: lucide(Terminal, '#10b981'),
  zsh: lucide(Terminal, '#10b981'),
  ps1: lucide(Terminal, '#10b981'),
  bat: lucide(Terminal, '#10b981'),
  cmd: lucide(Terminal, '#10b981'),
  png: lucide(ImageIcon, '#ec4899'),
  jpg: lucide(ImageIcon, '#ec4899'),
  jpeg: lucide(ImageIcon, '#ec4899'),
  gif: lucide(ImageIcon, '#ec4899'),
  webp: lucide(ImageIcon, '#ec4899'),
  ico: lucide(ImageIcon, '#ec4899'),
  avif: lucide(ImageIcon, '#ec4899'),
  woff: lucide(Type, '#a855f7'),
  woff2: lucide(Type, '#a855f7'),
  ttf: lucide(Type, '#a855f7'),
  otf: lucide(Type, '#a855f7'),
  lock: lucide(Lock, DIM),
  env: lucide(KeyRound, '#eab308'),
  sql: lucide(Braces, '#38bdf8'),
};

function baseName(path: string): string {
  const i = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  return i >= 0 ? path.slice(i + 1) : path;
}

function resolveFileSpec(path: string): IconSpec | null {
  const name = baseName(path).toLowerCase();
  if (BY_NAME[name]) return BY_NAME[name];
  if (name === '.env' || name.startsWith('.env.')) return lucide(KeyRound, '#eab308');
  if (/^tsconfig.*\.json$/.test(name)) return lucide(SlidersHorizontal, '#3178c6');
  if (/^vite\.config\./.test(name)) return lucide(Zap, '#a855f7');
  if (/^(eslint|prettier|tailwind|postcss)\.?config|^\.(eslint|prettier)rc/.test(name)) {
    return lucide(SlidersHorizontal, SLATE);
  }
  const dot = name.lastIndexOf('.');
  if (dot < 0) return null;
  return BY_EXT[name.slice(dot + 1)] ?? null;
}

/** Lockfiles and similar generated files recede visually. */
export function isDimFile(path: string): boolean {
  const name = baseName(path).toLowerCase();
  return name.endsWith('.lock') || name === 'package-lock.json' || name === 'pnpm-lock.yaml';
}

export const FileIcon: React.FC<{ path: string; size?: number; className?: string }> = ({
  path,
  size = 15,
  className = '',
}) => {
  const spec = resolveFileSpec(path);
  if (!spec) {
    return <File size={size} className={`shrink-0 text-[var(--text-dim)] ${className}`} />;
  }
  if (spec.kind === 'lucide') {
    const { Icon, color } = spec;
    return <Icon size={size} strokeWidth={1.9} className={`shrink-0 ${className}`} style={{ color }} />;
  }
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      className={`shrink-0 ${className}`}
      aria-hidden="true"
    >
      <rect x="1" y="1.5" width="14" height="13" rx="3" fill={spec.color} fillOpacity="0.18" />
      <rect
        x="1"
        y="1.5"
        width="14"
        height="13"
        rx="3"
        fill="none"
        stroke={spec.color}
        strokeOpacity="0.55"
        strokeWidth="1"
      />
      <text
        x="8"
        y="11.4"
        textAnchor="middle"
        fontSize={spec.label.length > 1 ? 7.4 : 8.5}
        fontWeight="700"
        fontFamily="Inter Variable, system-ui, sans-serif"
        fill={spec.color}
      >
        {spec.label}
      </text>
    </svg>
  );
};

const FOLDER_ICONS: Record<string, LucideIcon> = {
  src: FolderCode,
  app: FolderCode,
  components: FolderTree,
  ui: FolderTree,
  backend: Server,
  server: Server,
  api: Server,
  tests: FolderCheck,
  test: FolderCheck,
  __tests__: FolderCheck,
  '.github': FolderGit,
  '.git': FolderGit,
  '.vscode': FolderCog,
  '.claude': FolderCog,
  config: FolderCog,
};

export const FolderIcon: React.FC<{
  name: string;
  open?: boolean;
  size?: number;
  className?: string;
}> = ({ name, open = false, size = 15, className = '' }) => {
  const Special = FOLDER_ICONS[name.toLowerCase()] as LucideIcon | undefined;
  const Icon = Special ?? (open ? FolderOpen : Folder);
  return (
    <Icon
      size={size}
      strokeWidth={1.9}
      className={`shrink-0 ${className}`}
      style={{ color: Special ? '#7aa2d6' : '#8b9bb4' }}
    />
  );
};
