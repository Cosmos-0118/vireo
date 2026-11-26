import { diffWordsWithSpace } from 'diff';

export interface CharRange {
  from: number;
  to: number;
}

interface PairRanges {
  left: CharRange[];
  right: CharRange[];
}

const MAX_LINE_LEN = 1200;
const CACHE_MAX = 2000;
const EMPTY: PairRanges = { left: [], right: [] };
const cache = new Map<string, PairRanges>();

/**
 * Word-level changed ranges for a removed/added line pair, for both sides from ONE
 * `diffWordsWithSpace` pass (split rows and paired unified rows used to run it once per side).
 */
export function wordDiffRanges(oldText: string, newText: string): PairRanges {
  if (oldText.length > MAX_LINE_LEN || newText.length > MAX_LINE_LEN) return EMPTY;
  const key = `${oldText.length}:${oldText}${newText}`;
  const hit = cache.get(key);
  if (hit) return hit;
  const left: CharRange[] = [];
  const right: CharRange[] = [];
  let lp = 0;
  let rp = 0;
  for (const p of diffWordsWithSpace(oldText, newText)) {
    const len = p.value.length;
    if (p.added) {
      if (len > 0) right.push({ from: rp, to: rp + len });
      rp += len;
    } else if (p.removed) {
      if (len > 0) left.push({ from: lp, to: lp + len });
      lp += len;
    } else {
      lp += len;
      rp += len;
    }
  }
  const res = { left, right };
  if (cache.size >= CACHE_MAX) cache.delete(cache.keys().next().value as string);
  cache.set(key, res);
  return res;
}

export function computeChangedRanges(oldText: string, newText: string, side: 'left' | 'right'): CharRange[] {
  return wordDiffRanges(oldText, newText)[side];
}
