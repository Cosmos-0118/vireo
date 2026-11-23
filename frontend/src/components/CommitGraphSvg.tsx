import React from 'react';
import type { GraphRowLayout, Swimlane } from '../utils/gitGraph';

/** Shared geometry so the graph column width and the rails always agree. */
const MIN_LANE_W = 4;

/**
 * Horizontal distance between lanes. Graph-wide (never per row) so rails stay aligned
 * between rows. When there are more lanes than `maxVisibleLanes`, spacing shrinks so every
 * lane still fits in the same width instead of being clipped.
 */
export function graphLaneSpacing(maxLanes: number, laneWidth: number, maxVisibleLanes: number) {
  if (maxLanes <= maxVisibleLanes) return laneWidth;
  return Math.max(MIN_LANE_W, Math.min(laneWidth, (laneWidth * maxVisibleLanes) / maxLanes));
}

export function graphColumnWidth(maxLanes: number, laneWidth: number, maxVisibleLanes: number) {
  const w = graphLaneSpacing(maxLanes, laneWidth, maxVisibleLanes);
  return w * (Math.max(1, maxLanes) + 1);
}

interface CommitGraphSvgProps {
  row: GraphRowLayout;
  maxLanes: number;
  laneWidth?: number;
  rowHeight?: number;
  /** Cap rendered lane columns so narrow sidebars stay readable */
  maxVisibleLanes?: number;
  /** Background behind the row, used to cut a gap ring around nodes */
  background?: string;
  /**
   * Size the SVG to this row's own lanes instead of the whole graph, so the text that
   * follows indents with the rails (VS Code's history view).
   */
  fitRow?: boolean;
}

const STROKE = 1.5;
const CURVE = 5;
const DASH = '2.5 2.5';

/**
 * One row of the commit graph. Geometry follows VS Code's swimlane renderer:
 * straight rails, quarter-arc merges/forks at the row midline, and short
 * S-bends when a lane shifts left after another lane ends.
 */
export const CommitGraphSvg: React.FC<CommitGraphSvgProps> = ({
  row,
  maxLanes,
  laneWidth: laneW = 12,
  rowHeight: H = 24,
  maxVisibleLanes = 8,
  background = 'var(--graph-row-bg, var(--bg-sidebar))',
  fitRow = false,
}) => {
  const rowLanes = Math.max(row.input.length, row.output.length, row.circleIndex + 1);
  const W = graphLaneSpacing(maxLanes, laneW, maxVisibleLanes);
  const width = W * (Math.max(1, fitRow ? rowLanes : maxLanes) + 1);
  const mid = H / 2;
  const R = Math.min(CURVE, mid - 1, W / 2);
  const x = (i: number) => W * (i + 1);
  const { commit, input, output, circleIndex } = row;

  const paths: Array<{ d: string; color: string; dashed?: boolean }> = [];

  let outIdx = 0;
  input.forEach((lane, index) => {
    if (lane.id === commit.hash) {
      if (index !== circleIndex) {
        // ╯ converge into the node from the right
        paths.push({
          d: `M ${x(index)} 0 A ${W} ${mid} 0 0 1 ${x(index) - W} ${mid} H ${x(circleIndex)}`,
          color: lane.color,
          dashed: lane.dashed,
        });
      } else {
        outIdx++;
      }
      return;
    }
    if (outIdx < output.length && lane.id === output[outIdx].id) {
      if (index === outIdx) {
        paths.push({
          d: `M ${x(index)} 0 V ${H}`,
          color: lane.color,
          dashed: lane.dashed,
        });
      } else {
        // lane shifts left after a lane to its left ended
        paths.push({
          d:
            `M ${x(index)} 0 V ${mid - R} A ${R} ${R} 0 0 1 ${x(index) - R} ${mid} ` +
            `H ${x(outIdx) + R} A ${R} ${R} 0 0 0 ${x(outIdx)} ${mid + R} V ${H}`,
          color: lane.color,
          dashed: lane.dashed,
        });
      }
      outIdx++;
    }
  });

  // ╮ fork out to extra parents (merge commits)
  for (let i = 1; i < commit.parents.length; i++) {
    let p = -1;
    for (let k = output.length - 1; k >= 0; k--) {
      if (output[k].id === commit.parents[i]) {
        p = k;
        break;
      }
    }
    if (p === -1) continue;
    paths.push({
      d: `M ${x(p) - W} ${mid} A ${W} ${mid} 0 0 1 ${x(p)} ${H} M ${x(p) - W} ${mid} H ${x(circleIndex)}`,
      color: output[p].color,
      dashed: output[p].dashed,
    });
  }

  const inputIndex = input.findIndex((l) => l.id === commit.hash);
  if (inputIndex !== -1) {
    paths.push({
      d: `M ${x(circleIndex)} 0 V ${mid}`,
      color: input[inputIndex].color,
      dashed: input[inputIndex].dashed,
    });
  }
  if (commit.parents.length > 0 && output[circleIndex]) {
    paths.push({
      d: `M ${x(circleIndex)} ${mid} V ${H}`,
      color: row.circleColor,
      dashed: output[circleIndex].dashed,
    });
  }

  const cx = x(circleIndex);
  const color = row.circleColor;

  let node: React.ReactNode;
  if (row.kind === 'working-tree') {
    node = (
      <circle
        cx={cx}
        cy={mid}
        r={4.5}
        fill={background}
        stroke="var(--modified-fg)"
        strokeWidth={1.6}
        strokeDasharray="2.2 1.6"
      />
    );
  } else if (row.isHead) {
    // HEAD: large ring around a solid dot
    node = (
      <g>
        <circle cx={cx} cy={mid} r={6} fill={background} stroke={color} strokeWidth={2} />
        <circle cx={cx} cy={mid} r={2.5} fill={color} />
      </g>
    );
  } else if (row.isMerge) {
    // merge commit: ring with a dot, smaller than HEAD
    node = (
      <g>
        <circle cx={cx} cy={mid} r={4.75} fill={background} stroke={color} strokeWidth={1.5} />
        <circle cx={cx} cy={mid} r={2} fill={color} />
      </g>
    );
  } else {
    node = <circle cx={cx} cy={mid} r={4} fill={color} />;
  }

  return (
    <svg
      width={width}
      height={H}
      viewBox={`0 0 ${width} ${H}`}
      className="shrink-0 block"
      aria-hidden="true"
    >
      {paths.map((p, i) => (
        <path
          key={i}
          d={p.d}
          fill="none"
          stroke={p.color}
          strokeWidth={STROKE}
          strokeLinecap={p.dashed ? 'butt' : 'round'}
          strokeDasharray={p.dashed ? DASH : undefined}
        />
      ))}
      {node}
    </svg>
  );
};

/**
 * Continues a row's outgoing rails down through content rendered under the row
 * (e.g. an expanded file list), so the graph never visually breaks.
 */
export const GraphRails: React.FC<{
  lanes: Swimlane[];
  /** Widest row of the whole graph, so spacing matches the rows above */
  maxLanes: number;
  laneWidth?: number;
  maxVisibleLanes?: number;
}> = ({ lanes, maxLanes, laneWidth = 12, maxVisibleLanes = 8 }) => {
  const W = graphLaneSpacing(maxLanes, laneWidth, maxVisibleLanes);
  return (
    <div className="absolute inset-y-0 left-0 pointer-events-none" aria-hidden="true">
      {lanes.map((lane, i) => (
        <div
          key={i}
          className="absolute inset-y-0"
          style={{
            left: W * (i + 1) - STROKE / 2,
            width: STROKE,
            background: lane.dashed
              ? `repeating-linear-gradient(to bottom, ${lane.color} 0 2.5px, transparent 2.5px 5px)`
              : lane.color,
          }}
        />
      ))}
    </div>
  );
};
