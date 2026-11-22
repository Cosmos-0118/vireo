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
}

export interface TaskItem {
  name: string;
  command: string;
  source: string;
}

export interface Checkpoint {
  id: string;
  label: string;
  tree_sha: string;
  created_at: string;
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

export interface BranchesRes {
  current: string;
  branches: string[];
  remote_branches: string[];
}

export interface CommandRes {
  ok: boolean;
  stdout: string;
  stderr: string;
}

export interface OpenTab {
  id: string;
  path: string;
  mode: 'diff' | 'code' | 'commit-diff' | 'compare-diff' | 'graph';
  staged?: boolean;
  line?: number;
  commitHash?: string;
  commitShortHash?: string;
  commitSubject?: string;
  commitAuthor?: string;
  commitDate?: string;
  oldPath?: string | null;
  commitFiles?: CommitFileChange[];
}

export interface AgentConfig {
  provider: 'openai' | 'anthropic' | 'gemini' | 'ollama';
  baseUrl: string;
  apiKey: string;
  model: string;
  autoCheckpoint: boolean;
}

export interface ToolCallTrace {
  id: string;
  name: string;
  args: Record<string, unknown>;
  result?: string;
}

export interface UiChatMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  tools?: ToolCallTrace[];
  checkpoint?: Checkpoint;
  error?: string;
  streaming?: boolean;
}

export interface CliToolStatus {
  id: string;
  label: string;
  cmd: string;
  installed: boolean;
  path?: string | null;
  install_hint: string;
}
