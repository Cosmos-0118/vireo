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

/**
 * Inset of the first lane from the left edge (and of the trailing gap after the last lane).
 * Never narrower than a HEAD node, so compressed graphs don't clip it.
 */
const lanePad = (spacing: number) => Math.max(spacing, 8);

/** Centre x of lane `i` for a given lane spacing. */
export const graphLaneX = (i: number, spacing: number) => lanePad(spacing) + spacing * i;

/** Width of a graph strip holding `lanes` lanes, spaced for a graph `maxLanes` wide. */
export function graphWidth(
  lanes: number,
  maxLanes: number,
  laneWidth: number,
  maxVisibleLanes: number
) {
  const w = graphLaneSpacing(maxLanes, laneWidth, maxVisibleLanes);
  return lanePad(w) * 2 + w * (Math.max(1, lanes) - 1);
}

/** Width of the full graph column (every row reserves the widest row's lanes). */
export function graphColumnWidth(maxLanes: number, laneWidth: number, maxVisibleLanes: number) {
  return graphWidth(maxLanes, maxLanes, laneWidth, maxVisibleLanes);
}

/** Lanes a single row occupies (VS Code sizes each history row to this). */
export const graphRowLanes = (row: GraphRowLayout) =>
  Math.max(row.input.length, row.output.length, row.circleIndex + 1);

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
const CURVE = 6;
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
  const W = graphLaneSpacing(maxLanes, laneW, maxVisibleLanes);
  const width = graphWidth(fitRow ? graphRowLanes(row) : maxLanes, maxLanes, laneW, maxVisibleLanes);
  const mid = H / 2;
  const R = Math.min(CURVE, mid - 1, W / 2);
  const x = (i: number) => graphLaneX(i, W);
  const { commit, input, output, circleIndex } = row;

  const paths: Array<{ d: string; color: string; dashed?: boolean }> = [];

  let outIdx = 0;
  input.forEach((lane, index) => {
    if (lane.id === commit.hash) {
      if (index !== circleIndex) {
        // ╯ another rail waiting for this commit converges into the node from the right
        paths.push({
          d: `M ${x(index)} 0 V ${mid - R} A ${R} ${R} 0 0 1 ${x(index) - R} ${mid} H ${x(circleIndex)}`,
          color: lane.color,
          dashed: lane.dashed,
        });
      } else if (row.slotFilled) {
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
        // rail shifts left after a rail to its left ended
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

  // Links from the node down to each parent rail: straight, forking right (╮),
  // or joining an existing rail on either side (╰ / ╯).
  for (const link of row.parentLinks) {
    const px = x(link.index);
    let d: string;
    if (link.index === circleIndex) d = `M ${x(circleIndex)} ${mid} V ${H}`;
    else if (link.index > circleIndex)
      d = `M ${x(circleIndex)} ${mid} H ${px - R} A ${R} ${R} 0 0 1 ${px} ${mid + R} V ${H}`;
    else d = `M ${x(circleIndex)} ${mid} H ${px + R} A ${R} ${R} 0 0 0 ${px} ${mid + R} V ${H}`;
    paths.push({ d, color: link.color, dashed: link.dashed });
  }

  const inputIndex = input.findIndex((l) => l.id === commit.hash);
  if (inputIndex !== -1) {
    paths.push({
      d: `M ${x(circleIndex)} 0 V ${mid}`,
      color: input[inputIndex].color,
      dashed: input[inputIndex].dashed,
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
            left: graphLaneX(i, W) - STROKE / 2,
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
