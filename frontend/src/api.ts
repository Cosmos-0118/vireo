import { trackGitWrite } from './utils/gitWrites';
import type {
  AgentConfig,
  BranchesRes,
  Checkpoint,
  RollbackRes,
  CliToolStatus,
  LocalModelsInfo,
  PermissionRequest,
  QuestionRequest,
  SessionPrepareRes,
  SlashCommand,
  SteerAppliedEvent,
  SteerCancelledEvent,
  SteerMode,
  SteerRes,
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
  TasksRes,
  ReleaseFileChange,
  ReleaseNotesRes,
  ReleaseOutcome,
  ReleaseActivity,
  ReleaseBody,
  ReleaseReport,
  ReleaseRunOpts,
  ReleaseStatus,
  TreeEntry,
  WorkspaceInfo,
} from './types';

export interface AskpassPrompt {
  id: string;
  prompt: string;
  /** Hide the typed text (passphrases, passwords). */
  secret: boolean;
}

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

function releaseRun<T>(action: string, opts: ReleaseRunOpts = {}): Promise<T> {
  return jsonFetch<T>('/api/release', {
    method: 'POST',
    body: JSON.stringify({ action, ...opts }),
  });
}

export interface RateWindow {
  used_percent: number;
  window_minutes: number;
  resets_at: number;
}

export interface AgentUsageRes {
  claude: {
    block_tokens: number;
    block_resets_at: number;
    day_tokens: number;
    ctx_tokens: number;
    ctx_window: number;
    model: string;
  } | null;
  codex: {
    day_tokens: number;
    ctx_tokens: number;
    ctx_window: number;
    primary: RateWindow | null;
    secondary: RateWindow | null;
  } | null;
}

export const api = {
  /** Agent token usage read from the CLIs' session logs. */
  usage: () => jsonFetch<AgentUsageRes>('/api/usage'),
  setWindowZoom: (factor: number) =>
    jsonFetch<{ factor: number; native: boolean }>('/api/window/zoom', {
      method: 'POST',
      body: JSON.stringify({ factor }),
    }),
  windowControl: (action: 'state' | 'minimize' | 'toggle_maximize' | 'close' | 'drag') =>
    jsonFetch<{ native: boolean; maximized: boolean; platform?: string }>('/api/window/control', {
      method: 'POST',
      body: JSON.stringify({ action }),
    }),
  /** Credential prompts from git/ssh (SSH key passphrase, HTTPS password) still awaiting the user. */
  askpassPending: () => jsonFetch<AskpassPrompt[]>('/api/askpass/pending'),
  /** Answer a credential prompt; `null` cancels it. */
  askpassAnswer: async (id: string, value: string | null) => {
    await fetch('/api/askpass/answer', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id, value }),
    });
  },
  /** Name of the shell a new terminal tab runs ("zsh", "bash", "PowerShell"). */
  shellInfo: () => jsonFetch<{ name: string }>('/api/terminal/shell'),
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
  createEntry: (path: string, isDir: boolean) =>
    jsonFetch<{ ok: boolean; path: string }>('/api/files/create', {
      method: 'POST',
      body: JSON.stringify({ path, is_dir: isDir }),
    }),
  renameEntry: (from: string, to: string) =>
    jsonFetch<{ ok: boolean; path: string }>('/api/files/rename', {
      method: 'POST',
      body: JSON.stringify({ from, to }),
    }),
  deleteEntry: (path: string) =>
    jsonFetch<{ ok: boolean }>('/api/files/delete', {
      method: 'POST',
      body: JSON.stringify({ path }),
    }),
  osOpen: (path: string, mode: 'reveal' | 'open') =>
    jsonFetch<{ ok: boolean }>('/api/files/os-open', {
      method: 'POST',
      body: JSON.stringify({ path, mode }),
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
  getTasks: () => jsonFetch<TasksRes>('/api/tasks'),
  saveCustomTask: (task: {
    id?: string;
    name: string;
    command: string;
    cwd?: string;
    category?: string;
    env?: Record<string, string>;
    make_default?: boolean;
  }) =>
    jsonFetch<{ ok: boolean; id: string }>('/api/tasks/custom', {
      method: 'POST',
      body: JSON.stringify(task),
    }),
  deleteCustomTask: (id: string) =>
    jsonFetch<{ ok: boolean }>(`/api/tasks/custom?id=${encodeURIComponent(id)}`, {
      method: 'DELETE',
    }),

  releaseStatus: () => releaseRun<ReleaseStatus>('status'),
  releasePreflight: (o: ReleaseRunOpts = {}) => releaseRun<ReleaseReport>('preflight', o),
  releaseNotes: (o: ReleaseRunOpts = {}) => releaseRun<ReleaseNotesRes>('notes', o),
  releaseBump: (version: string, o: ReleaseRunOpts = {}) =>
    releaseRun<{ version: string; dry_run: boolean; files: ReleaseFileChange[] }>('bump', { ...o, version }),
  releasePublish: (o: ReleaseRunOpts = {}) =>
    o.dry_run
      ? releaseRun<ReleaseOutcome>('publish', o)
      : trackGitWrite(() => releaseRun<ReleaseOutcome>('publish', o)),
  releaseVerify: (o: ReleaseRunOpts = {}) => releaseRun<ReleaseReport>('verify', o),
  releaseBody: (tag: string) => releaseRun<ReleaseBody>('body', { tag }),
  releaseActivity: () => releaseRun<{ items: ReleaseActivity[] }>('activity'),

  gitStatus: () => jsonFetch<GitStatusRes>('/api/git/status'),
  gitAutoFetch: () =>
    jsonFetch<{ ok: boolean; changed: boolean; stderr: string }>('/api/git/auto-fetch', {
      method: 'POST',
    }),
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
  gitPruneRemoteBranches: () =>
    jsonFetch<CommandRes>('/api/git/remote-prune', {
      method: 'POST',
      body: '{}',
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
  answerQuestion: (turnId: string, requestId: string, answers: Record<string, string[]>, cancelled = false) =>
    jsonFetch<{ ok: boolean }>('/api/agent/question', {
      method: 'POST',
      body: JSON.stringify({ turn_id: turnId, request_id: requestId, answers, cancelled }),
    }),
  /** Injects or queues a mid-turn steering prompt for the thread's active turn. */
  steerAgent: async (
    threadId: string,
    turnId: string | null,
    text: string,
    mode: SteerMode = 'steer'
  ): Promise<SteerRes> => {
    const res = await fetch('/api/agent/steer', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        thread_id: threadId,
        turn_id: turnId || undefined,
        text,
        mode,
      }),
    });
    if (res.status === 409) {
      try {
        return (await res.json()) as SteerRes;
      } catch {
        return { accepted: false, reason: 'turn_ended' };
      }
    }
    if (!res.ok) {
      const txt = await res.text();
      throw new Error(txt || `HTTP ${res.status}`);
    }
    return (await res.json()) as SteerRes;
  },
  /** Withdraws a queued steering prompt before the agent consumes it. */
  cancelSteer: (threadId: string, turnId: string | null, steerId: string) =>
    jsonFetch<{ cancelled: boolean }>('/api/agent/steer/cancel', {
      method: 'POST',
      body: JSON.stringify({
        thread_id: threadId,
        turn_id: turnId || undefined,
        steer_id: steerId,
      }),
    }),
  /** Starts (or resumes) the thread's agent session and returns its slash commands. */
  prepareSession: (req: {
    threadId: string;
    provider: string;
    model: string;
    permissionMode?: string;
    resumeId?: string;
    allowedRules?: string[];
  }) =>
    jsonFetch<SessionPrepareRes>('/api/agent/session/prepare', {
      method: 'POST',
      body: JSON.stringify({
        thread_id: req.threadId,
        provider: req.provider,
        model: req.model,
        permission_mode: req.permissionMode,
        resume_id: req.resumeId,
        allowed_rules: req.allowedRules,
      }),
    }),
  /** Ends the thread's agent process (the thread was deleted). */
  closeSession: (threadId: string) =>
    jsonFetch<{ ok: boolean }>('/api/agent/session/close', {
      method: 'POST',
      body: JSON.stringify({ thread_id: threadId }),
    }),
  cliTools: () => jsonFetch<CliToolStatus[]>('/api/agent/cli-tools'),
  /** API keys live in the OS credential store (via the backend), never in browser storage. */
  getSecrets: (names: string[]) =>
    jsonFetch<Record<string, string>>(`/api/secrets?names=${encodeURIComponent(names.join(','))}`),
  putSecret: async (name: string, value: string) => {
    const res = await fetch(`/api/secrets/${encodeURIComponent(name)}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ value }),
    });
    if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
  },
  /** Live model list from an API provider (needs the key for Anthropic / Gemini). */
  apiModels: async (provider: string, baseUrl: string, apiKey: string): Promise<string[]> => {
    const res = await fetch('/api/agent/api-models', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ provider, base_url: baseUrl, api_key: apiKey }),
    });
    if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
    return ((await res.json()) as { models: string[] }).models;
  },
  /** Stores a pasted/dropped file on disk and returns its absolute path for the agent to read. */
  saveAttachment: async (blob: Blob, ext: string, name?: string): Promise<{ path: string }> => {
    const qs = `ext=${encodeURIComponent(ext)}${name ? `&name=${encodeURIComponent(name)}` : ''}`;
    const res = await fetch(`/api/agent/attachment?${qs}`, {
      method: 'POST',
      headers: { 'Content-Type': blob.type || 'application/octet-stream' },
      body: blob,
    });
    if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
    return res.json();
  },
  agentModels: (ollamaBase?: string, probeOllama = true) => {
    const params = new URLSearchParams();
    if (ollamaBase) params.set('ollama_base', ollamaBase);
    if (!probeOllama) params.set('probe_ollama', 'false');
    const qs = params.toString();
    return jsonFetch<LocalModelsInfo>(`/api/agent/models${qs ? `?${qs}` : ''}`);
  },
};

// Git ops that rewrite working-tree files: their watcher events are ours, not the agent's.
const GIT_WRITE_OPS = [
  'gitDiscard',
  'gitDiscardAll',
  'gitPull',
  'gitCheckout',
  'gitCherryPick',
  'gitRevert',
  'gitReset',
  'gitMerge',
  'gitStash',
  'gitOperation',
  'gitApplyPatch',
] as const satisfies ReadonlyArray<keyof typeof api>;
for (const name of GIT_WRITE_OPS) {
  const op = api[name] as (...args: unknown[]) => Promise<unknown>;
  (api as Record<string, unknown>)[name] = (...args: unknown[]) => trackGitWrite(() => op(...args));
}

export async function streamAgentChat(
  config: AgentConfig,
  messages: Array<{ role: string; content: string }>,
  handlers: {
    onText: (text: string) => void;
    /** A streamed fragment of the reply; consecutive fragments continue the same paragraph. */
    onTextDelta?: (text: string) => void;
    onToolCall: (call: { id: string; name: string; args: Record<string, unknown> }) => void;
    onToolResult: (res: { id: string; name: string; result: string }) => void;
    onCheckpoint: (cp: Checkpoint) => void;
    onPermissionRequest?: (req: PermissionRequest) => void;
    onPermissionCancel?: (requestId: string) => void;
    onUserQuestion?: (req: QuestionRequest) => void;
    /** Active turn id emitted at turn start so mid-turn steering can target it. */
    onTurn?: (turnId: string) => void;
    /** A mid-turn steer prompt was absorbed by the agent at this point in the timeline. */
    onSteerApplied?: (ev: SteerAppliedEvent) => void;
    /** A queued steer prompt was cancelled before being consumed. */
    onSteerCancelled?: (ev: SteerCancelledEvent) => void;
    /** The agent's own conversation id, to resume this thread after a restart. */
    onSession?: (s: { resume_id: string; resumed?: boolean }) => void;
    onCommands?: (commands: SlashCommand[]) => void;
    onError: (msg: string) => void;
    onDone: () => void;
  },
  signal?: AbortSignal,
  allowedRules?: string[],
  session?: { threadId: string; resumeId?: string }
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
      thread_id: session?.threadId,
      resume_id: session?.resumeId,
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
  let scanFrom = 0; // `\n\n` cannot start before this: never rescan the unterminated tail
  const currentEvent = 'message';
  let doneCalled = false;
  const emitDone = () => {
    if (!doneCalled) {
      doneCalled = true;
      handlers.onDone();
    }
  };

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });

    const parts: string[] = [];
    let start = 0;
    for (let idx = buffer.indexOf('\n\n', scanFrom); idx !== -1; idx = buffer.indexOf('\n\n', start)) {
      parts.push(buffer.slice(start, idx));
      start = idx + 2;
    }
    buffer = buffer.slice(start);
    scanFrom = Math.max(0, buffer.length - 1); // a split `\n` | `\n` can straddle chunks

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
        else if (evName === 'text_delta' && data.text) (handlers.onTextDelta ?? handlers.onText)(data.text);
        else if (evName === 'tool_call') handlers.onToolCall(data);
        else if (evName === 'tool_result') handlers.onToolResult(data);
        else if (evName === 'checkpoint') handlers.onCheckpoint(data);
        else if (evName === 'permission_request') handlers.onPermissionRequest?.(data);
        else if (evName === 'permission_cancel') handlers.onPermissionCancel?.(data.request_id);
        else if (evName === 'user_question') handlers.onUserQuestion?.(data);
        else if (evName === 'turn' && data.turn_id) handlers.onTurn?.(data.turn_id);
        else if (evName === 'steer_applied') handlers.onSteerApplied?.(data);
        else if (evName === 'steer_cancelled') handlers.onSteerCancelled?.(data);
        else if (evName === 'session' && data.resume_id) handlers.onSession?.(data);
        else if (evName === 'commands') handlers.onCommands?.(data.commands ?? []);
        else if (evName === 'error') handlers.onError(data.message || 'Unknown error');
        else if (evName === 'done') emitDone();
      } catch {
        // ignore malformed chunk
      }
    }
  }
  emitDone();
}

export function wsUrl(path: string): string {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${window.location.host}${path}`;
}
