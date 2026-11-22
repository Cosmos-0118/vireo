import type { CommitLogItem, GitRef } from '../types';

/**
 * Swimlane commit-graph layout modelled on VS Code's Source Control Graph
 * (src/vs/workbench/contrib/scm/browser/scmHistory.ts).
 *
 * Every row has a list of input lanes (rails entering from the top edge) and
 * output lanes (rails leaving through the bottom edge). Each lane is waiting
 * for a specific commit id. When a commit is reached, every lane waiting for
 * it converges into its node and is replaced by a single lane for its first
 * parent; remaining parents are appended on the right. Lanes to the right of
 * a converged lane shift left, so the graph stays compact.
 */

/** Lane colour of the checked-out branch (matches --accent). */
export const GRAPH_REF_COLOR = '#3b82f6';
/** Lane colour of the current branch's remote-tracking ref. */
export const GRAPH_REMOTE_REF_COLOR = '#a78bfa';
/** Rotating palette for all other lanes (tuned for dark backgrounds). */
export const GRAPH_COLORS = ['#e8a317', '#e0559a', '#3fb6a8', '#f0883e', '#8b9cf7', '#5fb85f'];

/** Synthetic id of the "Uncommitted Changes" row injected above HEAD. */
export const WORKING_TREE_ID = '__working_tree__';

export interface Swimlane {
  /** Commit id this lane is waiting for */
  id: string;
  color: string;
  /** Lane belongs to the working-tree → HEAD connector */
  dashed?: boolean;
}

export type SyncState = 'outgoing' | 'incoming' | null;

export interface GraphRowLayout {
  commit: CommitLogItem;
  kind: 'commit' | 'working-tree';
  input: Swimlane[];
  output: Swimlane[];
  /** Column of this row's node */
  circleIndex: number;
  circleColor: string;
  isMerge: boolean;
  isHead: boolean;
  /** Not pushed to (outgoing) / not pulled from (incoming) the upstream */
  sync: SyncState;
}

export interface GitGraphResult {
  rows: GraphRowLayout[];
  /** Widest row in lanes (>= 1) */
  maxLanes: number;
  headHash: string | null;
  /** Hash of the upstream (remote-tracking) tip of the current branch, if loaded */
  upstreamHash: string | null;
}

export interface GraphLayoutOptions {
  /** Inject an "Uncommitted Changes" row connected to HEAD */
  workingTree?: boolean;
  /** Current branch name (used to colour its upstream ref) */
  currentBranch?: string;
  /** Draw nodes only, no rails (e.g. for filtered/search results) */
  flat?: boolean;
}

const isUpstreamRef = (r: GitRef, branch?: string) =>
  !!branch && r.kind === 'remote' && r.name.slice(r.name.indexOf('/') + 1) === branch;

function labelColor(commit: CommitLogItem, branch?: string): string | undefined {
  if (commit.refs.some((r) => r.is_current)) return GRAPH_REF_COLOR;
  if (commit.refs.some((r) => isUpstreamRef(r, branch))) return GRAPH_REMOTE_REF_COLOR;
  return undefined;
}

function reachable(start: string, byHash: Map<string, CommitLogItem>): Set<string> {
  const seen = new Set<string>();
  const stack = [start];
  while (stack.length) {
    const h = stack.pop()!;
    if (seen.has(h)) continue;
    const c = byHash.get(h);
    if (!c) continue;
    seen.add(h);
    for (const p of c.parents) stack.push(p);
  }
  return seen;
}

/** Appends a fetched page, dropping commits already loaded (guards against overlapping pages). */
export function appendUniqueCommits(prev: CommitLogItem[], next: CommitLogItem[]): CommitLogItem[] {
  const seen = new Set(prev.map((c) => c.hash));
  const fresh = next.filter((c) => !seen.has(c.hash));
  return fresh.length ? [...prev, ...fresh] : prev;
}

export function computeGitGraphLayout(
  commits: CommitLogItem[],
  opts: GraphLayoutOptions = {}
): GitGraphResult {
  const branch = opts.currentBranch;
  const head = commits.find((c) => c.is_head) || null;
  const headHash = head?.hash ?? null;

  // Incoming / outgoing relative to the upstream ref, computed over loaded commits.
  let upstream: CommitLogItem | null = null;
  for (const c of commits) {
    const refs = c.refs.filter((r) => isUpstreamRef(r, branch));
    if (refs.length) {
      if (!upstream || refs.some((r) => r.name.startsWith('origin/'))) upstream = c;
      if (refs.some((r) => r.name.startsWith('origin/'))) break;
    }
  }
  const sync = new Map<string, SyncState>();
  if (head && upstream && head.hash !== upstream.hash) {
    const byHash = new Map(commits.map((c) => [c.hash, c]));
    const fromHead = reachable(head.hash, byHash);
    const fromUp = reachable(upstream.hash, byHash);
    for (const h of fromHead) if (!fromUp.has(h)) sync.set(h, 'outgoing');
    for (const h of fromUp) if (!fromHead.has(h)) sync.set(h, 'incoming');
  }

  const items: Array<{ commit: CommitLogItem; kind: GraphRowLayout['kind'] }> = [];
  for (const c of commits) {
    if (opts.workingTree && head && c.hash === head.hash) {
      items.push({
        kind: 'working-tree',
        commit: {
          hash: WORKING_TREE_ID,
          short_hash: '',
          parents: [head.hash],
          refs: [],
          message: 'Uncommitted Changes',
          author: '',
          author_email: '',
          date: '',
          iso_date: '',
          timestamp: Date.now() / 1000,
          is_head: false,
        },
      });
    }
    items.push({ commit: c, kind: 'commit' });
  }

  const rows: GraphRowLayout[] = [];
  let maxLanes = 1;

  if (opts.flat) {
    for (const { commit, kind } of items) {
      const color = labelColor(commit, branch) ?? GRAPH_REF_COLOR;
      rows.push({
        commit,
        kind,
        input: [],
        output: [],
        circleIndex: 0,
        circleColor: color,
        isMerge: commit.parents.length > 1,
        isHead: commit.is_head,
        sync: sync.get(commit.hash) ?? null,
      });
    }
    return { rows, maxLanes, headHash, upstreamHash: upstream?.hash ?? null };
  }

  let colorIdx = -1;
  const nextColor = () => {
    colorIdx = (colorIdx + 1) % GRAPH_COLORS.length;
    return GRAPH_COLORS[colorIdx];
  };
  const byHashForColor = new Map(commits.map((c) => [c.hash, c]));

  let prevOutput: Swimlane[] = [];
  for (const { commit, kind } of items) {
    const input = prevOutput.map((l) => ({ ...l }));
    const output: Swimlane[] = [];
    const isWT = kind === 'working-tree';
    const label = isWT ? GRAPH_REF_COLOR : labelColor(commit, branch);
    const parents = commit.parents;

    let firstParentAdded = false;
    for (const lane of input) {
      if (lane.id === commit.hash) {
        if (!firstParentAdded && parents.length > 0) {
          output.push({ id: parents[0], color: label ?? lane.color, dashed: isWT || undefined });
          firstParentAdded = true;
        }
        continue;
      }
      output.push({ ...lane });
    }

    for (let i = firstParentAdded ? 1 : 0; i < parents.length; i++) {
      let color: string | undefined;
      if (i === 0) color = label;
      else {
        const parent = byHashForColor.get(parents[i]);
        color = parent ? labelColor(parent, branch) : undefined;
      }
      output.push({ id: parents[i], color: color ?? nextColor(), dashed: isWT || undefined });
    }

    const inputIndex = input.findIndex((l) => l.id === commit.hash);
    const circleIndex = inputIndex !== -1 ? inputIndex : input.length;
    // Root commits keep their incoming lane colour (the slot in `output` now
    // belongs to whichever neighbouring lane shifted left into it).
    const circleColor =
      parents.length === 0 && inputIndex !== -1
        ? (label ?? input[inputIndex].color)
        : circleIndex < output.length
          ? output[circleIndex].color
          : circleIndex < input.length
            ? input[circleIndex].color
            : (label ?? GRAPH_REF_COLOR);

    maxLanes = Math.max(maxLanes, input.length, output.length, circleIndex + 1);
    rows.push({
      commit,
      kind,
      input,
      output,
      circleIndex,
      circleColor,
      isMerge: parents.length > 1,
      isHead: commit.is_head,
      sync: sync.get(commit.hash) ?? null,
    });
    prevOutput = output;
  }

  return { rows, maxLanes, headHash, upstreamHash: upstream?.hash ?? null };
}


export interface RemoteHostInfo {
  provider: 'github' | 'gitlab' | 'bitbucket' | 'other';
  label: string;
  repoUrl: string;
  commitUrl: (hash: string) => string;
}

/**
 * Parses SSH or HTTPS git remote URLs into clickable web URLs for GitHub/GitLab/Bitbucket.
 */
export function parseRemoteUrl(remoteUrl: string): RemoteHostInfo | null {
  const raw = (remoteUrl || '').trim();
  if (!raw) return null;

  let host = '';
  let pathPart = '';

  // SSH format: git@github.com:owner/repo.git
  const scpMatch = raw.match(/^[^@]+@([^:]+):(.+)$/);
  if (scpMatch) {
    host = scpMatch[1].toLowerCase();
    pathPart = scpMatch[2];
  } else {
    try {
      const normalized = raw.replace(/^ssh:\/\//i, 'https://');
      const u = new URL(normalized);
      host = u.hostname.toLowerCase();
      pathPart = u.pathname.replace(/^\/+/, '');
    } catch {
      return null;
    }
  }

  pathPart = pathPart.replace(/\.git$/i, '').replace(/\/+$/, '');
  if (!host || !pathPart) return null;

  const repoUrl = `https://${host}/${pathPart}`;

  if (host.includes('github')) {
    return {
      provider: 'github',
      label: 'GitHub',
      repoUrl,
      commitUrl: (hash) => `${repoUrl}/commit/${hash}`,
    };
  }
  if (host.includes('gitlab')) {
    return {
      provider: 'gitlab',
      label: 'GitLab',
      repoUrl,
      commitUrl: (hash) => `${repoUrl}/-/commit/${hash}`,
    };
  }
  if (host.includes('bitbucket')) {
    return {
      provider: 'bitbucket',
      label: 'Bitbucket',
      repoUrl,
      commitUrl: (hash) => `${repoUrl}/commits/${hash}`,
    };
  }
  return {
    provider: 'other',
    label: host,
    repoUrl,
    commitUrl: (hash) => `${repoUrl}/commit/${hash}`,
  };
}
