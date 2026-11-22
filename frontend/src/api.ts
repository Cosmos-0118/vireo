import type {
  AgentConfig,
  Checkpoint,
  CommandRes,
  CommitLogItem,
  GitDiffRes,
  GitStatusRes,
  SearchMatch,
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
  searchFiles: (q: string) =>
    jsonFetch<SearchMatch[]>(`/api/files/search?q=${encodeURIComponent(q)}`),
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
  gitCommit: (message: string, stage_all = false) =>
    jsonFetch<CommandRes>('/api/git/commit', {
      method: 'POST',
      body: JSON.stringify({ message, stage_all }),
    }),
  gitPush: (set_upstream = false) =>
    jsonFetch<CommandRes>('/api/git/push', {
      method: 'POST',
      body: JSON.stringify({ set_upstream }),
    }),
  gitPull: () =>
    jsonFetch<CommandRes>('/api/git/pull', {
      method: 'POST',
      body: '{}',
    }),
  gitBranches: () => jsonFetch<{ current: string; branches: string[] }>('/api/git/branches'),
  gitCheckout: (branch: string, create = false) =>
    jsonFetch<CommandRes>('/api/git/checkout', {
      method: 'POST',
      body: JSON.stringify({ branch, create }),
    }),
  gitLog: () => jsonFetch<CommitLogItem[]>('/api/git/log'),
  gitCheckpoint: (label?: string) =>
    jsonFetch<Checkpoint>('/api/git/checkpoint', {
      method: 'POST',
      body: JSON.stringify({ label }),
    }),
  gitRollback: (id: string) =>
    jsonFetch<CommandRes>('/api/git/rollback', {
      method: 'POST',
      body: JSON.stringify({ id }),
    }),
};

export async function streamAgentChat(
  config: AgentConfig,
  messages: Array<{ role: string; content: string }>,
  handlers: {
    onText: (text: string) => void;
    onToolCall: (call: { id: string; name: string; args: Record<string, unknown> }) => void;
    onToolResult: (res: { id: string; name: string; result: string }) => void;
    onCheckpoint: (cp: Checkpoint) => void;
    onError: (msg: string) => void;
    onDone: () => void;
  },
  signal?: AbortSignal
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
  let currentEvent = 'message';

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
