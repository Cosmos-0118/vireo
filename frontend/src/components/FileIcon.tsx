import React from 'react';
import {
  Atom,
  Binary,
  Blocks,
  Braces,
  Code,
  Container,
  Database,
  Film,
  File,
  FileArchive,
  FileDiff,
  FileKey,
  FileSpreadsheet,
  FileText,
  Music,
  NotebookText,
  Palette,
  Presentation,
  ScrollText,
  Workflow,
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
  'justfile': lucide(Terminal, '#10b981'),
  'procfile': lucide(Terminal, '#10b981'),
  'cmakelists.txt': lucide(Blocks, '#60a5fa'),
  'go.mod': badge('Go', '#06b6d4'),
  'go.sum': lucide(Lock, DIM),
  'gemfile': badge('Rb', '#ef4444'),
  'gemfile.lock': lucide(Lock, DIM),
  'rakefile': badge('Rb', '#ef4444'),
  'requirements.txt': badge('Py', '#3b82f6'),
  'pipfile': badge('Py', '#3b82f6'),
  'pipfile.lock': lucide(Lock, DIM),
  'poetry.lock': lucide(Lock, DIM),
  'composer.json': badge('Ph', '#8b5cf6'),
  'composer.lock': lucide(Lock, DIM),
  'pom.xml': badge('Jv', '#ef4444'),
  'build.gradle': badge('Gr', '#06b6d4'),
  'build.gradle.kts': badge('Gr', '#06b6d4'),
  'settings.gradle': badge('Gr', '#06b6d4'),
  'directory.build.props': lucide(Blocks, '#a855f7'),
  'directory.packages.props': lucide(Blocks, '#a855f7'),
  'nuget.config': lucide(Blocks, '#a855f7'),
  'global.json': lucide(Blocks, '#a855f7'),
  'pubspec.yaml': badge('Da', '#38bdf8'),
  'pubspec.lock': lucide(Lock, DIM),
  'mix.exs': badge('Ex', '#a855f7'),
  'deno.json': badge('JS', '#22c55e'),
  'deno.jsonc': badge('JS', '#22c55e'),
  'deno.lock': lucide(Lock, DIM),
  '.dockerignore': lucide(Container, '#38bdf8'),
  'compose.yml': lucide(Container, '#38bdf8'),
  'compose.yaml': lucide(Container, '#38bdf8'),
  '.editorconfig': lucide(SlidersHorizontal, SLATE),
  '.npmrc': lucide(SlidersHorizontal, '#ef4444'),
  '.nvmrc': lucide(SlidersHorizontal, '#22c55e'),
  '.gitkeep': lucide(GitBranch, DIM),
  'changelog.md': lucide(FileText, '#22c55e'),
  'contributing.md': lucide(FileText, '#60a5fa'),
  'code_of_conduct.md': lucide(FileText, '#60a5fa'),
  'claude.md': lucide(FileText, '#f97316'),
  'agents.md': lucide(FileText, '#f97316'),
  'codeowners': lucide(FileKey, SLATE),
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
  sql: lucide(Database, '#38bdf8'),

  // .NET / JVM / native languages
  cs: badge('C#', '#a855f7'),
  csx: badge('C#', '#a855f7'),
  vb: badge('VB', '#3b82f6'),
  fs: badge('F#', '#38bdf8'),
  fsx: badge('F#', '#38bdf8'),
  fsi: badge('F#', '#38bdf8'),
  csproj: lucide(Blocks, '#a855f7'),
  vbproj: lucide(Blocks, '#3b82f6'),
  fsproj: lucide(Blocks, '#38bdf8'),
  sln: lucide(Blocks, '#a855f7'),
  slnx: lucide(Blocks, '#a855f7'),
  props: lucide(Blocks, '#a855f7'),
  targets: lucide(Blocks, '#a855f7'),
  nuspec: lucide(Blocks, '#a855f7'),
  razor: lucide(Code, '#a855f7'),
  cshtml: lucide(Code, '#a855f7'),
  vbhtml: lucide(Code, '#3b82f6'),
  xaml: lucide(Code, '#a855f7'),
  axaml: lucide(Code, '#a855f7'),
  resx: lucide(Code, '#a855f7'),
  scala: badge('Sc', '#ef4444'),
  sc: badge('Sc', '#ef4444'),
  groovy: badge('Gr', '#06b6d4'),
  gradle: badge('Gr', '#06b6d4'),
  kts: badge('Kt', '#a855f7'),
  clj: badge('Cj', '#22c55e'),
  cljs: badge('Cj', '#22c55e'),
  cljc: badge('Cj', '#22c55e'),
  cxx: badge('C+', '#60a5fa'),
  hxx: badge('H+', '#60a5fa'),
  hh: badge('H+', '#60a5fa'),
  cu: badge('Cu', '#22c55e'),
  cuh: badge('Cu', '#22c55e'),
  ino: badge('C+', '#10b981'),
  m: badge('M', '#60a5fa'),
  mm: badge('M+', '#60a5fa'),
  d: badge('D', '#ef4444'),
  zig: badge('Zg', '#f97316'),
  nim: badge('Ni', '#eab308'),
  cr: badge('Cr', '#94a3b8'),
  v: badge('V', '#3b82f6'),
  odin: badge('Od', '#3b82f6'),
  dart: badge('Da', '#38bdf8'),
  lua: badge('Lu', '#3b82f6'),
  pl: badge('Pl', '#60a5fa'),
  pm: badge('Pl', '#60a5fa'),
  r: badge('R', '#3b82f6'),
  rmd: badge('R', '#3b82f6'),
  jl: badge('Jl', '#a855f7'),
  ex: badge('Ex', '#a855f7'),
  exs: badge('Ex', '#a855f7'),
  heex: badge('Ex', '#a855f7'),
  erl: badge('Er', '#ef4444'),
  hrl: badge('Er', '#ef4444'),
  hs: badge('Hs', '#a855f7'),
  lhs: badge('Hs', '#a855f7'),
  elm: badge('Elm', '#38bdf8'),
  ml: badge('ML', '#f97316'),
  mli: badge('ML', '#f97316'),
  rkt: badge('Rk', '#ef4444'),
  lisp: badge('Lp', '#22c55e'),
  el: badge('El', '#a855f7'),
  scm: badge('Sm', '#ef4444'),
  sol: badge('So', '#94a3b8'),
  pas: badge('Pa', '#f97316'),
  f90: badge('Fo', '#a855f7'),
  f95: badge('Fo', '#a855f7'),
  for: badge('Fo', '#a855f7'),
  cob: badge('Cb', '#3b82f6'),
  cbl: badge('Cb', '#3b82f6'),
  asm: badge('As', '#94a3b8'),
  s: badge('As', '#94a3b8'),
  wat: badge('Wa', '#a855f7'),
  wasm: lucide(Binary, '#a855f7'),
  tcl: badge('Tc', '#f97316'),
  awk: lucide(Terminal, '#10b981'),
  fish: lucide(Terminal, '#10b981'),
  nu: lucide(Terminal, '#10b981'),
  psm1: lucide(Terminal, '#10b981'),
  psd1: lucide(Terminal, '#10b981'),
  vba: badge('VB', '#3b82f6'),
  gd: badge('Gd', '#3b82f6'),
  glsl: badge('Gl', '#60a5fa'),
  hlsl: badge('Hl', '#60a5fa'),
  wgsl: badge('Wg', '#60a5fa'),
  shader: badge('Sh', '#60a5fa'),
  tf: badge('Tf', '#a855f7'),
  tfvars: badge('Tf', '#a855f7'),
  hcl: badge('Hc', '#a855f7'),
  nix: badge('Nx', '#60a5fa'),
  cmake: lucide(Blocks, '#60a5fa'),
  mk: lucide(Terminal, '#10b981'),
  mak: lucide(Terminal, '#10b981'),
  bzl: lucide(Blocks, '#22c55e'),
  proto: lucide(Workflow, '#38bdf8'),
  graphql: lucide(Workflow, '#ec4899'),
  gql: lucide(Workflow, '#ec4899'),
  prisma: lucide(Database, '#38bdf8'),

  // Web frameworks, templates, markup
  vue: badge('Vu', '#22c55e'),
  svelte: badge('Sv', '#f97316'),
  astro: badge('As', '#f97316'),
  xhtml: lucide(Code, '#f97316'),
  xml: lucide(Code, '#f97316'),
  xsl: lucide(Code, '#f97316'),
  xslt: lucide(Code, '#f97316'),
  xsd: lucide(Code, '#f97316'),
  plist: lucide(Code, '#f97316'),
  wxs: lucide(Code, '#f97316'),
  ejs: lucide(Code, '#eab308'),
  erb: lucide(Code, '#ef4444'),
  hbs: lucide(Code, '#f97316'),
  handlebars: lucide(Code, '#f97316'),
  mustache: lucide(Code, '#f97316'),
  pug: lucide(Code, '#a16207'),
  jade: lucide(Code, '#a16207'),
  njk: lucide(Code, '#22c55e'),
  twig: lucide(Code, '#22c55e'),
  liquid: lucide(Code, '#22c55e'),
  jinja: lucide(Code, '#ef4444'),
  jinja2: lucide(Code, '#ef4444'),
  j2: lucide(Code, '#ef4444'),
  tpl: lucide(Code, SLATE),
  styl: lucide(Hash, '#22c55e'),
  pcss: lucide(Hash, '#ec4899'),
  postcss: lucide(Hash, '#ec4899'),
  coffee: badge('Cf', '#a16207'),
  wsdl: lucide(Code, '#f97316'),

  // Config & data
  properties: lucide(SlidersHorizontal, SLATE),
  editorconfig: lucide(SlidersHorizontal, SLATE),
  npmrc: lucide(SlidersHorizontal, '#ef4444'),
  csv: lucide(FileSpreadsheet, '#22c55e'),
  tsv: lucide(FileSpreadsheet, '#22c55e'),
  xls: lucide(FileSpreadsheet, '#22c55e'),
  xlsx: lucide(FileSpreadsheet, '#22c55e'),
  xlsm: lucide(FileSpreadsheet, '#22c55e'),
  ods: lucide(FileSpreadsheet, '#22c55e'),
  jsonl: lucide(Braces, '#eab308'),
  ndjson: lucide(Braces, '#eab308'),
  geojson: lucide(Braces, '#22c55e'),
  parquet: lucide(Database, '#38bdf8'),
  db: lucide(Database, '#38bdf8'),
  sqlite: lucide(Database, '#38bdf8'),
  sqlite3: lucide(Database, '#38bdf8'),
  mdb: lucide(Database, '#38bdf8'),
  accdb: lucide(Database, '#38bdf8'),
  mdf: lucide(Database, '#38bdf8'),
  ldf: lucide(Database, '#38bdf8'),
  bak: lucide(Database, DIM),
  ipynb: lucide(NotebookText, '#f97316'),
  log: lucide(ScrollText, SLATE),
  diff: lucide(FileDiff, '#22c55e'),
  patch: lucide(FileDiff, '#22c55e'),
  map: lucide(Braces, DIM),

  // Documents
  pdf: lucide(FileText, '#ef4444'),
  doc: lucide(FileText, '#3b82f6'),
  docx: lucide(FileText, '#3b82f6'),
  odt: lucide(FileText, '#3b82f6'),
  rtf: lucide(FileText, '#3b82f6'),
  ppt: lucide(Presentation, '#f97316'),
  pptx: lucide(Presentation, '#f97316'),
  odp: lucide(Presentation, '#f97316'),
  key: lucide(FileKey, '#eab308'),
  tex: lucide(FileText, '#22c55e'),
  bib: lucide(FileText, '#22c55e'),
  rst: lucide(FileText, '#60a5fa'),
  adoc: lucide(FileText, '#60a5fa'),
  asciidoc: lucide(FileText, '#60a5fa'),
  org: lucide(FileText, '#22c55e'),
  epub: lucide(FileText, '#a855f7'),
  markdown: lucide(FileText, '#60a5fa'),

  // Images, design, fonts
  bmp: lucide(ImageIcon, '#ec4899'),
  tif: lucide(ImageIcon, '#ec4899'),
  tiff: lucide(ImageIcon, '#ec4899'),
  heic: lucide(ImageIcon, '#ec4899'),
  heif: lucide(ImageIcon, '#ec4899'),
  jfif: lucide(ImageIcon, '#ec4899'),
  apng: lucide(ImageIcon, '#ec4899'),
  jxl: lucide(ImageIcon, '#ec4899'),
  cur: lucide(ImageIcon, '#ec4899'),
  icns: lucide(ImageIcon, '#ec4899'),
  raw: lucide(ImageIcon, '#ec4899'),
  psd: lucide(Palette, '#38bdf8'),
  ai: lucide(Palette, '#f97316'),
  eps: lucide(Palette, '#f97316'),
  fig: lucide(Palette, '#a855f7'),
  sketch: lucide(Palette, '#eab308'),
  xcf: lucide(Palette, '#22c55e'),
  eot: lucide(Type, '#a855f7'),

  // Audio & video
  mp3: lucide(Music, '#06b6d4'),
  wav: lucide(Music, '#06b6d4'),
  flac: lucide(Music, '#06b6d4'),
  ogg: lucide(Music, '#06b6d4'),
  oga: lucide(Music, '#06b6d4'),
  m4a: lucide(Music, '#06b6d4'),
  aac: lucide(Music, '#06b6d4'),
  wma: lucide(Music, '#06b6d4'),
  opus: lucide(Music, '#06b6d4'),
  aiff: lucide(Music, '#06b6d4'),
  mid: lucide(Music, '#06b6d4'),
  midi: lucide(Music, '#06b6d4'),
  mp4: lucide(Film, '#f43f5e'),
  m4v: lucide(Film, '#f43f5e'),
  mkv: lucide(Film, '#f43f5e'),
  mov: lucide(Film, '#f43f5e'),
  avi: lucide(Film, '#f43f5e'),
  wmv: lucide(Film, '#f43f5e'),
  webm: lucide(Film, '#f43f5e'),
  flv: lucide(Film, '#f43f5e'),
  mpg: lucide(Film, '#f43f5e'),
  mpeg: lucide(Film, '#f43f5e'),
  ogv: lucide(Film, '#f43f5e'),
  srt: lucide(FileText, '#f43f5e'),
  vtt: lucide(FileText, '#f43f5e'),

  // Archives & packages
  zip: lucide(FileArchive, '#eab308'),
  tar: lucide(FileArchive, '#eab308'),
  gz: lucide(FileArchive, '#eab308'),
  tgz: lucide(FileArchive, '#eab308'),
  bz2: lucide(FileArchive, '#eab308'),
  xz: lucide(FileArchive, '#eab308'),
  zst: lucide(FileArchive, '#eab308'),
  lz4: lucide(FileArchive, '#eab308'),
  rar: lucide(FileArchive, '#eab308'),
  '7z': lucide(FileArchive, '#eab308'),
  cab: lucide(FileArchive, '#eab308'),
  iso: lucide(FileArchive, '#eab308'),
  dmg: lucide(FileArchive, '#eab308'),
  jar: lucide(Package, '#ef4444'),
  war: lucide(Package, '#ef4444'),
  nupkg: lucide(Package, '#a855f7'),
  whl: lucide(Package, '#3b82f6'),
  gem: lucide(Package, '#ef4444'),
  crate: lucide(Package, '#f97316'),
  vsix: lucide(Package, '#3b82f6'),
  apk: lucide(Package, '#22c55e'),
  ipa: lucide(Package, '#60a5fa'),
  deb: lucide(Package, '#ef4444'),
  rpm: lucide(Package, '#ef4444'),
  msi: lucide(Package, '#60a5fa'),
  msix: lucide(Package, '#60a5fa'),
  appx: lucide(Package, '#60a5fa'),

  // Compiled / binary artifacts
  exe: lucide(Binary, '#94a3b8'),
  dll: lucide(Binary, '#94a3b8'),
  pdb: lucide(Binary, DIM),
  so: lucide(Binary, '#94a3b8'),
  dylib: lucide(Binary, '#94a3b8'),
  lib: lucide(Binary, '#94a3b8'),
  a: lucide(Binary, '#94a3b8'),
  o: lucide(Binary, DIM),
  obj: lucide(Binary, DIM),
  bin: lucide(Binary, '#94a3b8'),
  dat: lucide(Binary, '#94a3b8'),
  class: lucide(Binary, '#ef4444'),
  pyc: lucide(Binary, '#3b82f6'),
  pyd: lucide(Binary, '#3b82f6'),
  rlib: lucide(Binary, '#f97316'),
  node: lucide(Binary, '#22c55e'),

  // Certificates & keys
  pem: lucide(FileKey, '#eab308'),
  crt: lucide(FileKey, '#eab308'),
  cer: lucide(FileKey, '#eab308'),
  csr: lucide(FileKey, '#eab308'),
  pfx: lucide(FileKey, '#eab308'),
  p12: lucide(FileKey, '#eab308'),
  pub: lucide(FileKey, '#eab308'),
  gpg: lucide(FileKey, '#eab308'),
  asc: lucide(FileKey, '#eab308'),
  pgp: lucide(FileKey, '#eab308'),
  keystore: lucide(FileKey, '#eab308'),
  jks: lucide(FileKey, '#eab308'),
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
  return (
    name.endsWith('.lock') ||
    name === 'package-lock.json' ||
    name === 'pnpm-lock.yaml' ||
    name === 'go.sum'
  );
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
        fontFamily="Inter UI, Inter Variable, system-ui, sans-serif"
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
