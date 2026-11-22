import type { CommitLogItem } from '../types';

/** Vibrant, high-contrast GitHub/VS Code branch rail palette */
export const GRAPH_COLORS = [
  '#3b82f6', // Blue (primary)
  '#22c55e', // Emerald
  '#a855f7', // Purple
  '#f59e0b', // Amber
  '#ec4899', // Pink
  '#06b6d4', // Cyan
  '#f97316', // Orange
  '#6366f1', // Indigo
  '#14b8a6', // Teal
  '#ef4444', // Red
];

export function graphColor(idx: number): string {
  return GRAPH_COLORS[Math.abs(idx) % GRAPH_COLORS.length];
}

export interface GraphSegment {
  fromLane: number;
  toLane: number;
  colorIdx: number;
}

export interface GraphRowLayout {
  commit: CommitLogItem;
  row: number;
  lane: number;
  colorIdx: number;
  isMerge: boolean;
  /** Vertical rails continuing uninterrupted through this row (y=0 -> y=rowH) */
  passThrough: Array<{ lane: number; colorIdx: number }>;
  /** Rails arriving from top edge (y=0) into this commit's node (lane, midY) */
  incoming: GraphSegment[];
  /** Rails leaving this commit's node (lane, midY) toward bottom edge (y=rowH) */
  outgoing: GraphSegment[];
  /** Highest active lane index touched in this row */
  maxLane: number;
}

export interface GitGraphResult {
  rows: GraphRowLayout[];
  maxLanes: number;
  headRowIndex: number;
  headLane: number;
  headColorIdx: number;
}

/**
 * Computes a topological swimlane layout for git commits (newest first).
 * Handles branch tips, linear history, branch splits (multiple children -> shared parent),
 * 2-way merges, octopus merges, root commits, and compact lane reuse.
 */
export function computeGitGraphLayout(commits: CommitLogItem[]): GitGraphResult {
  interface ActiveLane {
    commitHash: string;
    colorIdx: number;
  }

  const lanes: Array<ActiveLane | null> = [];
  const rows: GraphRowLayout[] = [];
  let nextColor = 0;
  let globalMaxLane = 0;
  let headRowIndex = -1;
  let headLane = 0;
  let headColorIdx = 0;

  const allocColor = (avoid?: number): number => {
    let c = nextColor++ % GRAPH_COLORS.length;
    if (avoid !== undefined && c === avoid) {
      c = nextColor++ % GRAPH_COLORS.length;
    }
    return c;
  };

  const findFreeLane = (preferAfter?: number): number => {
    if (preferAfter !== undefined) {
      for (let i = preferAfter + 1; i < lanes.length; i++) {
        if (lanes[i] === null) return i;
      }
    }
    const firstNull = lanes.indexOf(null);
    if (firstNull !== -1) return firstNull;
    lanes.push(null);
    return lanes.length - 1;
  };

  for (let row = 0; row < commits.length; row++) {
    const commit = commits[row];
    const incoming: GraphSegment[] = [];
    const outgoing: GraphSegment[] = [];
    const passThrough: Array<{ lane: number; colorIdx: number }> = [];

    // 1. Find all incoming active lanes expecting this commit
    const matchingLanes: number[] = [];
    for (let l = 0; l < lanes.length; l++) {
      if (lanes[l]?.commitHash === commit.hash) {
        matchingLanes.push(l);
      }
    }

    let nodeLane: number;
    let nodeColor: number;

    if (matchingLanes.length > 0) {
      nodeLane = matchingLanes[0];
      nodeColor = lanes[nodeLane]!.colorIdx;
      for (const l of matchingLanes) {
        incoming.push({
          fromLane: l,
          toLane: nodeLane,
          colorIdx: lanes[l]!.colorIdx,
        });
        lanes[l] = null;
      }
    } else {
      nodeLane = findFreeLane();
      nodeColor = allocColor();
    }

    if (commit.is_head && headRowIndex === -1) {
      headRowIndex = row;
      headLane = nodeLane;
      headColorIdx = nodeColor;
    }

    // 2. Remaining non-null lanes pass straight through this row
    for (let l = 0; l < lanes.length; l++) {
      const active = lanes[l];
      if (active !== null) {
        passThrough.push({ lane: l, colorIdx: active.colorIdx });
      }
    }

    // 3. Assign outgoing rails for parents
    const parents = commit.parents || [];
    if (parents.length > 0) {
      // Primary parent (first parent) stays on nodeLane unless another active lane already targets it
      const p0 = parents[0];
      const existingP0 = lanes.findIndex((x) => x?.commitHash === p0);
      if (existingP0 !== -1) {
        outgoing.push({
          fromLane: nodeLane,
          toLane: existingP0,
          colorIdx: nodeColor,
        });
      } else {
        lanes[nodeLane] = { commitHash: p0, colorIdx: nodeColor };
        outgoing.push({
          fromLane: nodeLane,
          toLane: nodeLane,
          colorIdx: nodeColor,
        });
      }

      // Secondary parents (merge commit) branch out smoothly
      for (let k = 1; k < parents.length; k++) {
        const pk = parents[k];
        const existingPk = lanes.findIndex((x) => x?.commitHash === pk);
        if (existingPk !== -1) {
          outgoing.push({
            fromLane: nodeLane,
            toLane: existingPk,
            colorIdx: lanes[existingPk]!.colorIdx,
          });
        } else {
          const pLane = findFreeLane(nodeLane);
          const branchColor = allocColor(nodeColor);
          lanes[pLane] = { commitHash: pk, colorIdx: branchColor };
          outgoing.push({
            fromLane: nodeLane,
            toLane: pLane,
            colorIdx: branchColor,
          });
        }
      }
    }

    // 4. Trim trailing empty lanes so width stays compact
    while (lanes.length > 0 && lanes[lanes.length - 1] === null) {
      lanes.pop();
    }

    let rowMaxLane = nodeLane;
    for (const p of passThrough) rowMaxLane = Math.max(rowMaxLane, p.lane);
    for (const s of incoming) rowMaxLane = Math.max(rowMaxLane, s.fromLane, s.toLane);
    for (const s of outgoing) rowMaxLane = Math.max(rowMaxLane, s.fromLane, s.toLane);
    globalMaxLane = Math.max(globalMaxLane, rowMaxLane);

    rows.push({
      commit,
      row,
      lane: nodeLane,
      colorIdx: nodeColor,
      isMerge: parents.length > 1,
      passThrough,
      incoming,
      outgoing,
      maxLane: rowMaxLane,
    });
  }

  return {
    rows,
    maxLanes: Math.max(1, globalMaxLane + 1),
    headRowIndex,
    headLane,
    headColorIdx,
  };
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
