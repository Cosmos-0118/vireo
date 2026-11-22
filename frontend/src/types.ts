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
  ahead: number;
  behind: number;
  remote_url: string;
  files: GitFileStatus[];
  total_additions: number;
  total_deletions: number;
  checkpoints: Checkpoint[];
}

export interface GitDiffRes {
  path: string;
  staged: boolean;
  original: string;
  modified: string;
  patch: string;
  binary: boolean;
}

export interface CommitLogItem {
  hash: string;
  short_hash: string;
  message: string;
  author: string;
  date: string;
}

export interface CommandRes {
  ok: boolean;
  stdout: string;
  stderr: string;
}

export interface OpenTab {
  path: string;
  mode: 'diff' | 'code';
  staged?: boolean;
  line?: number;
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
