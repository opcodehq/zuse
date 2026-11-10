// Commit graph lanes, ported from crates/protocol/src/presentation/git_history.rs (layout_graph).
// Commits arrive child-before-parent; active lanes point at the parent that will resolve each path.
import type { GitHistoryCommit } from "./types";

export type SegmentShape = "through" | "incoming" | "outgoing";

export interface GraphSegment {
  fromLane: number;
  toLane: number;
  colorId: number;
  shape: SegmentShape;
}

export interface GraphRow {
  sha: string;
  nodeLane: number;
  nodeColorId: number;
  segments: GraphSegment[];
  isHead: boolean;
}

export interface GraphLayout {
  rows: GraphRow[];
  maxLaneCount: number;
}

interface Lane {
  id: number;
  colorId: number;
  target: string;
}

export function layoutGraph(commits: readonly GitHistoryCommit[], headSha: string | null | undefined): GraphLayout {
  let active: Lane[] = [];
  let nextLaneId = 0;
  let nextColorId = 0;
  let maxLaneCount = 0;
  const rows: GraphRow[] = [];
  const indexOf = (lanes: Lane[], id: number) => lanes.findIndex((lane) => lane.id === id);

  for (const commit of commits) {
    const before = active;
    const incoming: number[] = [];
    before.forEach((lane, index) => lane.target === commit.sha && incoming.push(index));
    const primaryIncoming = incoming[0];
    const nodeLane = primaryIncoming ?? before.length;
    const primaryLane = primaryIncoming !== undefined ? before[primaryIncoming] : undefined;
    const nodeColorId = primaryLane ? primaryLane.colorId : nextColorId++;
    const resolved = new Set(incoming.map((index) => before[index]!.id));
    const next = before.filter((lane) => !resolved.has(lane.id));
    const outgoing: [number, number][] = [];

    let primaryOutgoing: number | null = null;
    const firstParent = commit.parentShas[0];
    if (firstParent !== undefined) {
      const id = primaryLane ? primaryLane.id : nextLaneId++;
      next.splice(Math.min(nodeLane, next.length), 0, { id, colorId: nodeColorId, target: firstParent });
      primaryOutgoing = id;
      outgoing.push([id, nodeColorId]);
    }

    let offset = 1;
    for (const parent of commit.parentShas.slice(1)) {
      const existing = next.find((lane) => lane.target === parent);
      if (existing) {
        outgoing.push([existing.id, existing.colorId]);
        continue;
      }
      const lane = { id: nextLaneId++, colorId: nextColorId++, target: parent };
      const primaryIndex = primaryOutgoing !== null ? indexOf(next, primaryOutgoing) : Math.min(nodeLane, next.length);
      next.splice(Math.min(primaryIndex + offset, next.length), 0, lane);
      offset++;
      outgoing.push([lane.id, lane.colorId]);
    }

    const segments: GraphSegment[] = [];
    before.forEach((lane, fromLane) => {
      if (!resolved.has(lane.id)) segments.push({ fromLane, toLane: indexOf(next, lane.id), colorId: lane.colorId, shape: "through" });
    });
    for (const fromLane of incoming) segments.push({ fromLane, toLane: nodeLane, colorId: before[fromLane]!.colorId, shape: "incoming" });
    for (const [id, colorId] of outgoing) segments.push({ fromLane: nodeLane, toLane: indexOf(next, id), colorId, shape: "outgoing" });

    maxLaneCount = Math.max(maxLaneCount, before.length, next.length, nodeLane + 1);
    rows.push({ sha: commit.sha, nodeLane, nodeColorId, segments, isHead: headSha === commit.sha });
    active = next;
  }
  return { rows, maxLaneCount };
}
