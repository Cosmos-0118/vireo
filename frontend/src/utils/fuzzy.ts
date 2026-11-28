/**
 * Fast, ranked fuzzy path search for Quick Open.
 *
 * - The lowercase path / basename offset are computed once per file list, not per keystroke.
 * - Matching is an allocation-free char-code scan; results are ranked (basename, prefix,
 *   word-boundary and consecutive-run bonuses) and kept in a bounded top-N buffer.
 * - Typing a query that extends the previous one only rescans the previous matches, since every
 *   match of the longer query is also a match of its prefix.
 */

export interface FuzzySearcher {
  /** Returns up to `limit` best-matching paths for `query` (already trimmed/lowercased). */
  search(query: string, limit: number): string[];
}

const isSep = (c: number) => c === 47 /* / */ || c === 92 /* \ */;
const isBoundary = (c: number) =>
  c === 47 || c === 92 || c === 46 /* . */ || c === 45 /* - */ || c === 95 /* _ */ || c === 32;

export function createFuzzySearcher(files: string[]): FuzzySearcher {
  const n = files.length;
  const lower = new Array<string>(n);
  const baseStart = new Int32Array(n);
  for (let i = 0; i < n; i++) {
    const l = files[i].toLowerCase();
    lower[i] = l;
    let b = l.length - 1;
    while (b >= 0 && !isSep(l.charCodeAt(b))) b--;
    baseStart[i] = b + 1;
  }

  let prevQuery = '';
  let prevMatches: Int32Array | null = null; // indexes matching prevQuery (null = all files)

  /** Score of `q` against file `i`, or -1 when it does not match. */
  function score(i: number, q: string): number {
    const s = lower[i];
    const orig = files[i];
    const base = baseStart[i];
    const len = s.length;
    const qlen = q.length;

    // Exact substring beats any scattered subsequence.
    const at = s.lastIndexOf(q);
    if (at >= 0) {
      let sc = 1000;
      if (at >= base) sc += 400; // entirely within the file name
      if (at === base) sc += 300; // file name starts with the query
      else if (at === 0) sc += 100;
      else if (isBoundary(s.charCodeAt(at - 1))) sc += 150;
      if (len - base === qlen) sc += 200; // whole file name
      return sc - len; // shorter paths first
    }

    // Subsequence with run / boundary bonuses.
    let qi = 0;
    let sc = 0;
    let run = 0;
    for (let k = 0; k < len && qi < qlen; k++) {
      const c = s.charCodeAt(k);
      if (c === q.charCodeAt(qi)) {
        qi++;
        run++;
        sc += 10 + run * 5;
        if (k === 0 || isBoundary(s.charCodeAt(k - 1))) sc += 30;
        else if (orig.charCodeAt(k) !== c && orig.charCodeAt(k - 1) === s.charCodeAt(k - 1)) sc += 30; // camelCase hump
        if (k >= base) sc += 8;
      } else {
        run = 0;
      }
    }
    return qi === qlen ? sc - len : -1;
  }

  return {
    search(query, limit) {
      if (!query) return files.slice(0, limit);

      const canNarrow = prevMatches !== null && query.startsWith(prevQuery);
      const total = canNarrow ? prevMatches!.length : n;
      const matched = new Int32Array(total);
      let m = 0;

      const topIdx = new Int32Array(limit);
      const topScore = new Float64Array(limit);
      let top = 0;

      for (let j = 0; j < total; j++) {
        const i = canNarrow ? prevMatches![j] : j;
        const sc = score(i, query);
        if (sc < 0) continue;
        matched[m++] = i;
        // Bounded insertion into the sorted (desc) top-N buffer. Strict `>` keeps list order on ties.
        if (top === limit && sc <= topScore[top - 1]) continue;
        let p = top < limit ? top++ : top - 1;
        while (p > 0 && topScore[p - 1] < sc) {
          topScore[p] = topScore[p - 1];
          topIdx[p] = topIdx[p - 1];
          p--;
        }
        topScore[p] = sc;
        topIdx[p] = i;
      }

      prevQuery = query;
      prevMatches = matched.subarray(0, m);

      const out = new Array<string>(top);
      for (let k = 0; k < top; k++) out[k] = files[topIdx[k]];
      return out;
    },
  };
}
