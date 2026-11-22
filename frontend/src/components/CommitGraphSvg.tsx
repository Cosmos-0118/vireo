import React from 'react';
import { graphColor, type GraphRowLayout } from '../utils/gitGraph';

interface CommitGraphSvgProps {
  row: GraphRowLayout;
  maxLanes: number;
  laneWidth?: number;
  rowHeight?: number;
  nodeRadius?: number;
  /** Cap rendered lane columns so narrow sidebars stay readable */
  maxVisibleLanes?: number;
}

export const CommitGraphSvg: React.FC<CommitGraphSvgProps> = ({
  row,
  maxLanes,
  laneWidth = 14,
  rowHeight = 28,
  nodeRadius = 3.6,
  maxVisibleLanes = 8,
}) => {
  const lanesCount = Math.max(1, Math.min(maxLanes, maxVisibleLanes));
  const padX = Math.max(8, Math.round(laneWidth * 0.65));
  const width = padX * 2 + Math.max(0, lanesCount - 1) * laneWidth;
  const midY = rowHeight / 2;

  const xOf = (lane: number) => padX + Math.min(lane, maxVisibleLanes - 1) * laneWidth;
  const nodeX = xOf(row.lane);
  const nodeColor = graphColor(row.colorIdx);
  const strokeWidth = 1.75;

  return (
    <svg
      width={width}
      height={rowHeight}
      viewBox={`0 0 ${width} ${rowHeight}`}
      className="shrink-0 block overflow-visible"
      aria-hidden="true"
    >
      {/* 1. Uninterrupted vertical pass-through rails */}
      {row.passThrough.map((pt) => {
        const x = xOf(pt.lane);
        return (
          <line
            key={`pt-${pt.lane}`}
            x1={x}
            y1={0}
            x2={x}
            y2={rowHeight}
            stroke={graphColor(pt.colorIdx)}
            strokeWidth={strokeWidth}
            strokeLinecap="round"
            opacity={0.85}
          />
        );
      })}

      {/* 2. Incoming rails from top edge (y=0) to node center */}
      {row.incoming.map((seg, idx) => {
        const x1 = xOf(seg.fromLane);
        const x2 = xOf(seg.toLane);
        const color = graphColor(seg.colorIdx);
        if (seg.fromLane === seg.toLane) {
          return (
            <line
              key={`in-${idx}`}
              x1={x1}
              y1={0}
              x2={x2}
              y2={midY}
              stroke={color}
              strokeWidth={strokeWidth}
              strokeLinecap="round"
            />
          );
        }
        const d = `M ${x1} 0 C ${x1} ${midY * 0.62}, ${x2} ${midY * 0.35}, ${x2} ${midY}`;
        return (
          <path
            key={`in-${idx}`}
            d={d}
            fill="none"
            stroke={color}
            strokeWidth={strokeWidth}
            strokeLinecap="round"
          />
        );
      })}

      {/* 3. Outgoing rails from node center to bottom edge (y=rowHeight) */}
      {row.outgoing.map((seg, idx) => {
        const x1 = xOf(seg.fromLane);
        const x2 = xOf(seg.toLane);
        const color = graphColor(seg.colorIdx);
        if (seg.fromLane === seg.toLane) {
          return (
            <line
              key={`out-${idx}`}
              x1={x1}
              y1={midY}
              x2={x2}
              y2={rowHeight}
              stroke={color}
              strokeWidth={strokeWidth}
              strokeLinecap="round"
            />
          );
        }
        const dy = rowHeight - midY;
        const d = `M ${x1} ${midY} C ${x1} ${midY + dy * 0.65}, ${x2} ${midY + dy * 0.38}, ${x2} ${rowHeight}`;
        return (
          <path
            key={`out-${idx}`}
            d={d}
            fill="none"
            stroke={color}
            strokeWidth={strokeWidth}
            strokeLinecap="round"
          />
        );
      })}

      {/* 4. Commit node (HEAD highlight, Merge double-ring, or standard solid dot) */}
      {row.commit.is_head ? (
        <g>
          <circle
            cx={nodeX}
            cy={midY}
            r={nodeRadius + 2.8}
            fill={nodeColor}
            opacity={0.22}
          />
          <circle
            cx={nodeX}
            cy={midY}
            r={nodeRadius + 1.3}
            fill="var(--bg-sidebar)"
            stroke={nodeColor}
            strokeWidth={1.8}
          />
          <circle cx={nodeX} cy={midY} r={Math.max(1.8, nodeRadius - 1.2)} fill={nodeColor} />
        </g>
      ) : row.isMerge ? (
        <g>
          <circle
            cx={nodeX}
            cy={midY}
            r={nodeRadius + 1.1}
            fill="var(--bg-sidebar)"
            stroke={nodeColor}
            strokeWidth={1.8}
          />
          <circle cx={nodeX} cy={midY} r={1.6} fill={nodeColor} />
        </g>
      ) : (
        <circle
          cx={nodeX}
          cy={midY}
          r={nodeRadius}
          fill={nodeColor}
          stroke="var(--bg-sidebar)"
          strokeWidth={1.4}
        />
      )}
    </svg>
  );
};

interface WorkingTreeGraphSvgProps {
  maxLanes: number;
  headLane: number;
  headColorIdx: number;
  laneWidth?: number;
  rowHeight?: number;
  maxVisibleLanes?: number;
}

/** Virtual top row representing local uncommitted changes connected to HEAD */
export const WorkingTreeGraphSvg: React.FC<WorkingTreeGraphSvgProps> = ({
  maxLanes,
  headLane,
  headColorIdx,
  laneWidth = 14,
  rowHeight = 28,
  maxVisibleLanes = 8,
}) => {
  const lanesCount = Math.max(1, Math.min(maxLanes, maxVisibleLanes));
  const padX = Math.max(8, Math.round(laneWidth * 0.65));
  const width = padX * 2 + Math.max(0, lanesCount - 1) * laneWidth;
  const midY = rowHeight / 2;
  const x = padX + Math.min(headLane, maxVisibleLanes - 1) * laneWidth;
  const color = graphColor(headColorIdx);

  return (
    <svg
      width={width}
      height={rowHeight}
      viewBox={`0 0 ${width} ${rowHeight}`}
      className="shrink-0 block"
      aria-hidden="true"
    >
      <line
        x1={x}
        y1={midY + 4}
        x2={x}
        y2={rowHeight}
        stroke={color}
        strokeWidth={1.6}
        strokeDasharray="2.5 2.5"
        opacity={0.8}
      />
      <circle
        cx={x}
        cy={midY}
        r={3.8}
        fill="var(--bg-sidebar)"
        stroke="var(--modified-fg)"
        strokeWidth={1.8}
        strokeDasharray="2 1.5"
      />
    </svg>
  );
};
