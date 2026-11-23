export interface WorkspaceInfo {
  path: string;
  name: string;
}

export interface TreeEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
}

export interface SearchMatch {
  path: string;
  line: number;
  column: number;
  text: string;
  /** Match ranges `[start, end)` in UTF-16 units relative to `text`. */
  ranges: [number, number][];
}

export interface SearchOptions {
  q: string;
  case_sensitive: boolean;
  whole_word: boolean;
  regex: boolean;
  include: string;
  exclude: string;
  use_gitignore: boolean;
  only_paths?: string[] | null;
}

export interface SearchRes {
  matches: SearchMatch[];
  total_matches: number;
  total_files: number;
  truncated: boolean;
}

export interface ReplaceRes {
  files_changed: number;
  replacements: number;
  skipped: string[];
}

export type TaskCategory = 'run' | 'build' | 'test' | 'check' | 'other';

export interface TaskItem {
  /** Stable id: `pkg:frontend:dev`, `custom:my-task`, `local:my-task`, `file:<path>`. */
  id: string;
  name: string;
  command: string;
  source: string;
  /** Working directory relative to the workspace root ('' = root). */
  cwd: string;
  category: TaskCategory;
  /** Backend-assigned rank; higher = more likely the primary run target. */
  score: number;
  /** Defined in .vireo/tasks.json. */
  custom: boolean;
  /** Personal task stored on this machine only. */
  local?: boolean;
  env: Record<string, string>;
  /** Ids of tasks to start together with this one (compound tasks). */
  depends_on: string[];
}

export interface TasksRes {
  tasks: TaskItem[];
  default_task: string | null;
}

/** Optional button shown inside a toast (e.g. "Redo" after an undo). */
export interface ToastAction {
  label: string;
  run: () => void;
}

export interface CheckpointFileDelta {
  path: string;
  before_blob: string | null;
  after_blob: string | null;
  additions: number;
  deletions: number;
}

export interface Checkpoint {
  id: string;
  label: string;
  tree_sha: string;
  created_at: string;
  /** "turn" is scoped to `files`; "manual" is a whole-workspace snapshot. */
  kind: 'turn' | 'manual';
  /** Only the files this turn changed. */
  files: CheckpointFileDelta[];
  /** Present once (part of) the turn was undone: what was replaced, so it can be redone. */
  redo_files: CheckpointFileDelta[] | null;
}

export interface RollbackRes {
  ok: boolean;
  stdout: string;
  stderr: string;
  restored: number;
  /** Files changed after the turn that were left alone. */
  conflicts: string[];
  checkpoint: Checkpoint | null;
}

export interface StashItem {
  index: string;
  message: string;
  date: string;
}

export interface GitFileStatus {
  path: string;
  index_status: string;
  worktree_status: string;
  staged: boolean;
  unstaged: boolean;
  untracked: boolean;
  additions: number;
  deletions: number;
}

export interface GitStatusRes {
  is_repo: boolean;
  branch: string;
  head_hash: string;
  ahead: number;
  behind: number;
  remote_url: string;
  state: 'cherry-pick' | 'merge' | 'revert' | 'rebase' | null;
  files: GitFileStatus[];
  total_additions: number;
  total_deletions: number;
  checkpoints: Checkpoint[];
  stashes: StashItem[];
}

export interface GenerateMessageRes {
  ok: boolean;
  message: string;
  code: 'copilot_missing' | 'copilot_unauthenticated' | 'no_changes' | 'failed' | '';
  error: string;
}

export interface GithubStatusRes {
  installed: boolean;
  authenticated: boolean;
  login: string;
}

export interface GitDiffRes {
  path: string;
  staged: boolean;
  original: string;
  modified: string;
  patch: string;
  binary: boolean;
}

export interface GitRef {
  name: string;
  kind: 'head' | 'local' | 'remote' | 'tag';
  is_current: boolean;
}

export interface CommitLogItem {
  hash: string;
  short_hash: string;
  parents: string[];
  refs: GitRef[];
  message: string;
  author: string;
  author_email: string;
  date: string;
  iso_date: string;
  timestamp: number;
  is_head: boolean;
}

export interface CommitLogRes {
  commits: CommitLogItem[];
  has_more: boolean;
  head_hash: string;
}

export interface CommitFileChange {
  path: string;
  old_path?: string | null;
  status: string;
  additions: number;
  deletions: number;
  binary: boolean;
}

export interface CommitDetailRes {
  hash: string;
  short_hash: string;
  parents: string[];
  refs: GitRef[];
  subject: string;
  body: string;
  author: string;
  author_email: string;
  date: string;
  iso_date: string;
  files: CommitFileChange[];
  total_additions: number;
  total_deletions: number;
}

export interface BranchInfo {
  name: string;
  kind: 'local' | 'remote';
  is_current: boolean;
  upstream: string;
  ahead: number;
  behind: number;
  upstream_gone: boolean;
  timestamp: number;
  relative_date: string;
  subject: string;
}

export interface BranchesRes {
  current: string;
  branches: string[];
  remote_branches: string[];
  /** Newer backends only; newest commit first */
  details?: BranchInfo[];
  upstream?: string;
  upstream_gone?: boolean;
  detached?: boolean;
  head_short?: string;
  default_branch?: string;
  remotes?: string[];
  last_fetched?: number | null;
}

export interface CommandRes {
  ok: boolean;
  stdout: string;
  stderr: string;
}

export interface OpenTab {
  id: string;
  path: string;
  mode: 'diff' | 'code' | 'commit-diff' | 'compare-diff' | 'graph' | 'settings';
  staged?: boolean;
  line?: number;
  col?: number;
  colEnd?: number;
  jumpKey?: number;
  commitHash?: string;
  commitShortHash?: string;
  commitSubject?: string;
  commitAuthor?: string;
  commitDate?: string;
  oldPath?: string | null;
  commitFiles?: CommitFileChange[];
}

export interface AgentConfig {
  provider:
    | 'openai'
    | 'anthropic'
    | 'gemini'
    | 'ollama'
    | 'claude-cli'
    | 'codex-cli'
    | 'copilot-cli';
  baseUrl: string;
  apiKey: string;
  model: string;
  autoCheckpoint: boolean;
  /** Installed CLI providers only. Defaults to 'auto_edit'. */
  permissionMode?: PermissionMode;
}

export type PermissionMode = 'ask' | 'auto_edit' | 'full_access';

/** A tool-approval prompt from a running Claude Code turn, shown inline until answered. */
export interface PermissionRequest {
  turn_id: string;
  request_id: string;
  tool: string;
  summary: string;
  input: Record<string, unknown>;
  /** Rule saved when the user picks "always allow in this thread". */
  rule: string;
}

/** An entry of the composer's `/` palette, discovered from the running agent session. */
export interface SlashCommand {
  /** Without the leading slash. */
  name: string;
  description: string;
  /** Argument hint such as `<file>`. */
  hint?: string;
  source: 'builtin' | 'skill' | 'project' | 'user' | string;
}

export interface QuestionOption {
  label: string;
  description: string;
}

export interface AgentQuestion {
  /** Key of the answer sent back to the agent. */
  id: string;
  header: string;
  question: string;
  options: QuestionOption[];
  multi_select: boolean;
  /** The user may type a free-form answer instead of picking an option. */
  allow_other: boolean;
  secret: boolean;
}

/** Multiple-choice questions from a running agent turn, shown inline until answered. */
export interface QuestionRequest {
  turn_id: string;
  request_id: string;
  questions: AgentQuestion[];
}

/** Result of warming a thread's agent session. */
export interface SessionPrepareRes {
  resume_id: string | null;
  resumed?: boolean;
  commands: SlashCommand[];
}

export interface ToolCallTrace {
  id: string;
  name: string;
  args: Record<string, unknown>;
  result?: string;
  /** Client-side timing (epoch ms), stamped when the call and its result arrive. */
  startedAt?: number;
  endedAt?: number;
}

/** Chronological layout of an assistant turn: prose and tool runs in the order they happened. */
export type MessagePart = { kind: 'text'; text: string } | { kind: 'tools'; ids: string[] };

export interface UiChatMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  tools?: ToolCallTrace[];
  /** Assistant turns: render order. Older saved threads lack it and fall back to tools-then-text. */
  parts?: MessagePart[];
  checkpoint?: Checkpoint;
  error?: string;
  streaming?: boolean;
  /** User turns: context attached by the composer (shown as badges, never as raw text). */
  ctxFiles?: string[];
  ctxChanges?: number;
  /** User turns: number of pasted images attached (the paths ride in the prompt, not the bubble). */
  ctxImages?: number;
  /** Assistant turns: timing (epoch ms) for the "Thought for Ns" row. */
  startedAt?: number;
  firstOutputAt?: number;
  /** Engine label ("Claude Code · sonnet") the turn ran on, for the turn header. */
  engine?: string;
  endedAt?: number;
}

export interface LocalModelsInfo {
  ollama: { running: boolean; models: string[] };
  /** Model slugs listed in the user's Codex install. */
  codex: string[];
}

export interface CliToolStatus {
  id: string;
  label: string;
  cmd: string;
  installed: boolean;
  path?: string | null;
  install_hint: string;
}
