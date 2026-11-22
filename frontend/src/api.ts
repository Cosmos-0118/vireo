import type {
  AgentConfig,
  BranchesRes,
  Checkpoint,
  RollbackRes,
  CliToolStatus,
  LocalModelsInfo,
  PermissionRequest,
  CommandRes,
  CommitDetailRes,
  CommitFileChange,
  CommitLogRes,
  GitDiffRes,
  GitStatusRes,
  GenerateMessageRes,
  GithubStatusRes,
  ReplaceRes,
  SearchOptions,
  SearchRes,
  TaskItem,
  TreeEntry,
  WorkspaceInfo,
} from './types';

async function jsonFetch<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, {
    headers: { 'Content-Type': 'application/json', ...(init?.headers || {}) },
    ...init,
  });
  if (!res.ok) {
    const txt = await res.text();
    throw new Error(txt || `HTTP ${res.status}`);
  }
  return res.json() as Promise<T>;
}

export const api = {
  setWindowZoom: (factor: number) =>
    jsonFetch<{ factor: number; native: boolean }>('/api/window/zoom', {
      method: 'POST',
      body: JSON.stringify({ factor }),
    }),
  windowControl: (action: 'state' | 'minimize' | 'toggle_maximize' | 'close' | 'drag') =>
    jsonFetch<{ native: boolean; maximized: boolean }>('/api/window/control', {
      method: 'POST',
      body: JSON.stringify({ action }),
    }),
  getWorkspace: () => jsonFetch<WorkspaceInfo>('/api/workspace'),
  setWorkspace: (path: string) =>
    jsonFetch<WorkspaceInfo>('/api/workspace', {
      method: 'POST',
      body: JSON.stringify({ path }),
    }),
  pickWorkspace: () =>
    jsonFetch<WorkspaceInfo | null>('/api/workspace/pick', {
      method: 'POST',
      body: '{}',
    }),
  getTree: (path = '') =>
    jsonFetch<TreeEntry[]>(`/api/files/tree?path=${encodeURIComponent(path)}`),
  listFiles: () => jsonFetch<string[]>('/api/files/list'),
  readFile: (path: string) =>
    jsonFetch<{ path: string; content: string; size: number; binary: boolean }>(
      `/api/files/read?path=${encodeURIComponent(path)}`
    ),
  writeFile: (path: string, content: string) =>
    jsonFetch<{ ok: boolean }>('/api/files/write', {
      method: 'POST',
      body: JSON.stringify({ path, content }),
    }),
  searchFiles: (opts: SearchOptions, signal?: AbortSignal) =>
    jsonFetch<SearchRes>('/api/files/search', {
      method: 'POST',
      body: JSON.stringify(opts),
      signal,
    }),
  replaceInFiles: (opts: {
    q: string;
    case_sensitive: boolean;
    whole_word: boolean;
    regex: boolean;
    replacement: string;
    targets: { path: string; lines?: number[] | null }[];
  }) =>
    jsonFetch<ReplaceRes>('/api/files/replace', {
      method: 'POST',
      body: JSON.stringify(opts),
    }),
  getTasks: () => jsonFetch<TaskItem[]>('/api/tasks'),

  gitStatus: () => jsonFetch<GitStatusRes>('/api/git/status'),
  gitDiff: (path: string, staged = false) =>
    jsonFetch<GitDiffRes>(
      `/api/git/diff?path=${encodeURIComponent(path)}&staged=${staged ? 'true' : 'false'}`
    ),
  gitStage: (opts: { paths?: string[]; all?: boolean }) =>
    jsonFetch<CommandRes>('/api/git/stage', {
      method: 'POST',
      body: JSON.stringify(opts),
    }),
  gitUnstage: (opts: { paths?: string[]; all?: boolean }) =>
    jsonFetch<CommandRes>('/api/git/unstage', {
      method: 'POST',
      body: JSON.stringify(opts),
    }),
  gitDiscard: (path: string, untracked: boolean) =>
    jsonFetch<CommandRes>('/api/git/discard', {
      method: 'POST',
      body: JSON.stringify({ path, untracked }),
    }),
  gitDiscardAll: (include_untracked = false) =>
    jsonFetch<CommandRes>('/api/git/discard-all', {
      method: 'POST',
      body: JSON.stringify({ include_untracked }),
    }),
  gitCommit: (message: string, stage_all = false, amend = false) =>
    jsonFetch<CommandRes>('/api/git/commit', {
      method: 'POST',
      body: JSON.stringify({ message, stage_all, amend }),
    }),
  gitInit: (branch?: string) =>
    jsonFetch<CommandRes>('/api/git/init', {
      method: 'POST',
      body: JSON.stringify(branch ? { branch } : {}),
    }),
  githubStatus: () => jsonFetch<GithubStatusRes>('/api/git/github-status'),
  gitGenerateMessage: (amend = false) =>
    jsonFetch<GenerateMessageRes>('/api/git/generate-message', {
      method: 'POST',
      body: JSON.stringify({ amend }),
    }),
  gitPublishGithub: (name: string, isPrivate: boolean, description?: string) =>
    jsonFetch<CommandRes>('/api/git/publish-github', {
      method: 'POST',
      body: JSON.stringify({ name, private: isPrivate, description: description || undefined }),
    }),
  gitRemoteAdd: (url: string, name = 'origin') =>
    jsonFetch<CommandRes>('/api/git/remote-add', {
      method: 'POST',
      body: JSON.stringify({ name, url }),
    }),
  gitPush: (set_upstream = false) =>
    jsonFetch<CommandRes>('/api/git/push', {
      method: 'POST',
      body: JSON.stringify({ set_upstream }),
    }),
  gitPull: (mode?: 'merge' | 'rebase' | 'ff-only') =>
    jsonFetch<CommandRes>('/api/git/pull', {
      method: 'POST',
      body: JSON.stringify(mode ? { mode } : {}),
    }),
  gitFetch: () =>
    jsonFetch<CommandRes>('/api/git/fetch', {
      method: 'POST',
      body: '{}',
    }),
  gitBranches: () => jsonFetch<BranchesRes>('/api/git/branches'),
  gitCheckout: (branch: string, create = false, start_point?: string) =>
    jsonFetch<CommandRes>('/api/git/checkout', {
      method: 'POST',
      body: JSON.stringify({ branch, create, start_point }),
    }),
  gitDeleteBranch: (name: string, force = false) =>
    jsonFetch<CommandRes>('/api/git/branch-delete', {
      method: 'POST',
      body: JSON.stringify({ name, force }),
    }),
  gitLog: (opts?: {
    limit?: number;
    skip?: number;
    all?: boolean;
    search?: string;
    branch?: string;
  }) => {
    const params = new URLSearchParams();
    if (opts?.limit !== undefined) params.set('limit', String(opts.limit));
    if (opts?.skip !== undefined) params.set('skip', String(opts.skip));
    if (opts?.all !== undefined) params.set('all', opts.all ? 'true' : 'false');
    if (opts?.search) params.set('search', opts.search);
    if (opts?.branch) params.set('branch', opts.branch);
    const qs = params.toString();
    return jsonFetch<CommitLogRes>(`/api/git/log${qs ? `?${qs}` : ''}`);
  },
  gitCommitDetail: (hash: string) =>
    jsonFetch<CommitDetailRes>(`/api/git/commit-detail?hash=${encodeURIComponent(hash)}`),
  gitCommitDiff: (hash: string, path: string, oldPath?: string | null) => {
    const params = new URLSearchParams({ hash, path });
    if (oldPath) params.set('old_path', oldPath);
    return jsonFetch<GitDiffRes>(`/api/git/commit-diff?${params.toString()}`);
  },
  gitCommitPatch: (hash: string) =>
    jsonFetch<{ hash: string; patch: string }>(
      `/api/git/commit-patch?hash=${encodeURIComponent(hash)}`
    ),
  gitCompareFiles: (base: string) =>
    jsonFetch<CommitFileChange[]>(
      `/api/git/compare-files?base=${encodeURIComponent(base)}`
    ),
  gitCompareDiff: (base: string, path: string, oldPath?: string | null) => {
    const params = new URLSearchParams({ base, path });
    if (oldPath) params.set('old_path', oldPath);
    return jsonFetch<GitDiffRes>(`/api/git/compare-diff?${params.toString()}`);
  },
  gitCherryPick: (hash: string, no_commit = false) =>
    jsonFetch<CommandRes>('/api/git/cherry-pick', {
      method: 'POST',
      body: JSON.stringify({ hash, no_commit }),
    }),
  gitRevert: (hash: string, no_commit = false) =>
    jsonFetch<CommandRes>('/api/git/revert', {
      method: 'POST',
      body: JSON.stringify({ hash, no_commit }),
    }),
  gitReset: (target: string, mode: 'soft' | 'mixed' | 'hard') =>
    jsonFetch<CommandRes>('/api/git/reset', {
      method: 'POST',
      body: JSON.stringify({ target, mode }),
    }),
  gitMerge: (target: string) =>
    jsonFetch<CommandRes>('/api/git/merge', {
      method: 'POST',
      body: JSON.stringify({ target }),
    }),
  gitCreateTag: (name: string, target: string, message?: string) =>
    jsonFetch<CommandRes>('/api/git/tag', {
      method: 'POST',
      body: JSON.stringify({ name, target, message }),
    }),
  gitStash: (
    action: 'push' | 'pop' | 'apply' | 'drop',
    opts?: { message?: string; index?: string }
  ) =>
    jsonFetch<CommandRes>('/api/git/stash', {
      method: 'POST',
      body: JSON.stringify({ action, ...opts }),
    }),
  gitOperation: (
    kind: 'cherry-pick' | 'merge' | 'revert' | 'rebase',
    action: 'continue' | 'abort'
  ) =>
    jsonFetch<CommandRes>('/api/git/operation', {
      method: 'POST',
      body: JSON.stringify({ kind, action }),
    }),
  gitApplyPatch: (patch: string, cached = false, reverse = false) =>
    jsonFetch<CommandRes>('/api/git/apply-patch', {
      method: 'POST',
      body: JSON.stringify({ patch, cached, reverse }),
    }),
  openExternalUrl: (url: string) =>
    jsonFetch<CommandRes>('/api/git/open-external', {
      method: 'POST',
      body: JSON.stringify({ url }),
    }),
  gitCheckpoint: (label?: string) =>
    jsonFetch<Checkpoint>('/api/git/checkpoint', {
      method: 'POST',
      body: JSON.stringify({ label }),
    }),
  gitRollback: (id: string, force = false) =>
    jsonFetch<RollbackRes>('/api/git/rollback', {
      method: 'POST',
      body: JSON.stringify({ id, force }),
    }),
  gitRedo: (id: string, force = false) =>
    jsonFetch<RollbackRes>('/api/git/redo', {
      method: 'POST',
      body: JSON.stringify({ id, force }),
    }),
  answerPermission: (turnId: string, requestId: string, allow: boolean, remember = false) =>
    jsonFetch<{ ok: boolean }>('/api/agent/permission', {
      method: 'POST',
      body: JSON.stringify({ turn_id: turnId, request_id: requestId, allow, remember }),
    }),
  cliTools: () => jsonFetch<CliToolStatus[]>('/api/agent/cli-tools'),
  agentModels: (ollamaBase?: string) =>
    jsonFetch<LocalModelsInfo>(
      `/api/agent/models${ollamaBase ? `?ollama_base=${encodeURIComponent(ollamaBase)}` : ''}`
    ),
};

export async function streamAgentChat(
  config: AgentConfig,
  messages: Array<{ role: string; content: string }>,
  handlers: {
    onText: (text: string) => void;
    onToolCall: (call: { id: string; name: string; args: Record<string, unknown> }) => void;
    onToolResult: (res: { id: string; name: string; result: string }) => void;
    onCheckpoint: (cp: Checkpoint) => void;
    onPermissionRequest?: (req: PermissionRequest) => void;
    onPermissionCancel?: (requestId: string) => void;
    onError: (msg: string) => void;
    onDone: () => void;
  },
  signal?: AbortSignal,
  allowedRules?: string[]
) {
  const res = await fetch('/api/agent/chat', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    signal,
    body: JSON.stringify({
      provider: config.provider,
      base_url: config.baseUrl || undefined,
      api_key: config.apiKey || undefined,
      model: config.model,
      messages,
      auto_checkpoint: config.autoCheckpoint,
      permission_mode: config.permissionMode ?? 'auto_edit',
      allowed_rules: allowedRules?.length ? allowedRules : undefined,
    }),
  });

  if (!res.ok || !res.body) {
    handlers.onError(`Agent request failed (${res.status})`);
    handlers.onDone();
    return;
  }

  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  const currentEvent = 'message';

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });

    const parts = buffer.split('\n\n');
    buffer = parts.pop() || '';

    for (const block of parts) {
      let evName = currentEvent;
      let dataStr = '';
      for (const line of block.split('\n')) {
        if (line.startsWith('event:')) {
          evName = line.slice(6).trim();
        } else if (line.startsWith('data:')) {
          dataStr += line.slice(5).trim();
        }
      }
      if (!dataStr) continue;
      try {
        const data = JSON.parse(dataStr);
        if (evName === 'text' && data.text) handlers.onText(data.text);
        else if (evName === 'tool_call') handlers.onToolCall(data);
        else if (evName === 'tool_result') handlers.onToolResult(data);
        else if (evName === 'checkpoint') handlers.onCheckpoint(data);
        else if (evName === 'permission_request') handlers.onPermissionRequest?.(data);
        else if (evName === 'permission_cancel') handlers.onPermissionCancel?.(data.request_id);
        else if (evName === 'error') handlers.onError(data.message || 'Unknown error');
        else if (evName === 'done') handlers.onDone();
      } catch {
        // ignore malformed chunk
      }
    }
  }
  handlers.onDone();
}

export function wsUrl(path: string): string {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${window.location.host}${path}`;
}
