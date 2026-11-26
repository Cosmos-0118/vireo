// Frontend micro-benchmarks (node, no DOM). Run: npm run bench
//   Each case times the pre-optimization logic ("before", copied from git history) against the
//   code now in src/ ("after").
import { diffLines, diffWordsWithSpace } from 'diff';
import React from 'react';
import { renderToString } from 'react-dom/server';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { computeChangedRanges } from '../src/utils/wordDiff';
import { highlightLinesByName } from '../src/syntax';
import { BUILTIN_THEMES } from '../src/themes';

type Row = { name: string; before: number; after: number };
const rows: Row[] = [];
const now = () => performance.now();
function best(runs: number, f: () => void): number {
  let b = Infinity;
  for (let i = 0; i < runs; i++) {
    const t = now();
    f();
    b = Math.min(b, now() - t);
  }
  return b;
}
const add = (name: string, before: number, after: number) => rows.push({ name, before, after });

// ---- 1. DiffViewer word diff: both sides of a pair --------------------------------------
{
  const pairs: Array<[string, string]> = [];
  for (let i = 0; i < 3000; i++) {
    const a = `    const result_${i} = computeSomething(argumentOne, argumentTwo, ${i}) + otherValue_${i};`;
    const b = `    const result_${i} = computeSomethingElse(argumentOne, argumentThree, ${i + 1}) + otherValue_${i};`;
    pairs.push([a, b]);
  }
  const legacy = (o: string, n: string, side: 'left' | 'right') => {
    if (o.length > 1200 || n.length > 1200) return [];
    const ranges: Array<{ from: number; to: number }> = [];
    let pos = 0;
    for (const p of diffWordsWithSpace(o, n)) {
      if (side === 'left') {
        if (p.added) continue;
        if (p.removed && p.value.length > 0) ranges.push({ from: pos, to: pos + p.value.length });
        pos += p.value.length;
      } else {
        if (p.removed) continue;
        if (p.added && p.value.length > 0) ranges.push({ from: pos, to: pos + p.value.length });
        pos += p.value.length;
      }
    }
    return ranges;
  };
  const before = best(3, () => {
    for (const [a, b] of pairs) {
      legacy(a, b, 'left');
      legacy(a, b, 'right');
    }
  });
  // Cold cache: run once (a second run would hit the memo and flatter the result).
  const after = best(1, () => {
    for (const [a, b] of pairs) {
      computeChangedRanges(a, b, 'left');
      computeChangedRanges(a, b, 'right');
    }
  });
  add('DiffViewer word-diff (3000 changed pairs, both sides)', before, after);
}

// ---- 2. App.tsx watcher: duplicate full-file diffLines on first edit ---------------------
{
  const mk = (edit: boolean) =>
    Array.from({ length: 30000 }, (_, i) => (edit && i % 1500 === 0 ? `changed line ${i}` : `line ${i}: some ordinary source text`)).join('\n');
  const prev = mk(false);
  const cur = mk(true);
  const count = () => {
    let adds = 0;
    let dels = 0;
    for (const p of diffLines(prev, cur)) {
      if (p.added) adds += p.count ?? 0;
      else if (p.removed) dels += p.count ?? 0;
    }
    return { adds, dels };
  };
  const before = best(3, () => {
    count();
    count(); // `total` pass: base === prev -> identical diff computed again
  });
  const after = best(3, () => {
    count(); // total reuses adds/dels when base === prev
  });
  add('App watcher diffLines (30k-line file, first edit)', before, after);
}

// ---- 3. AgentPanel filteredFiles on every render ----------------------------------------
{
  const allFiles = Array.from({ length: 4000 }, (_, i) => `src/pkg${i % 40}/module_${i}/Component${i}.tsx`);
  const extraFiles = Array.from({ length: 60 }, (_, i) => allFiles[i * 50]);
  const oldFn = (q0: string) => {
    const q = q0.trim().toLowerCase();
    const pool = allFiles.filter((f) => !extraFiles.includes(f));
    return (q ? pool.filter((f) => f.toLowerCase().includes(q)) : pool).slice(0, 40);
  };
  const newFn = (open: boolean, q0: string) => {
    if (!open) return [];
    const q = q0.trim().toLowerCase();
    const taken = new Set(extraFiles);
    const out: string[] = [];
    for (const f of allFiles) {
      if (taken.has(f) || (q && !f.toLowerCase().includes(q))) continue;
      out.push(f);
      if (out.length >= 40) break;
    }
    return out;
  };
  // 1000 renders (tokens) with the picker closed
  add(
    'AgentPanel filteredFiles x1000 renders (picker closed, 4000 files)',
    best(5, () => {
      for (let i = 0; i < 1000; i++) oldFn('');
    }),
    best(5, () => {
      for (let i = 0; i < 1000; i++) newFn(false, '');
    }),
  );
  add(
    'AgentPanel filteredFiles x100 keystrokes (picker open, query)',
    best(5, () => {
      for (let i = 0; i < 100; i++) oldFn('comp1');
    }),
    best(5, () => {
      for (let i = 0; i < 100; i++) newFn(true, 'comp1');
    }),
  );
}

// ---- 4. Streaming: markdown re-parse per token vs per animation frame ---------------------
{
  const para = 'The quick brown fox jumps over the lazy dog, then **refactors** the `parser` module. ';
  const code =
    '```ts\n' + Array.from({ length: 40 }, (_, i) => `const v${i} = compute(${i}, "x") + other${i};`).join('\n') + '\n```\n\n';
  const full = (para.repeat(6) + '\n\n').repeat(3) + code + (para.repeat(5) + '\n\n').repeat(2);
  const tokens: string[] = [];
  for (let i = 0; i < full.length; i += 4) tokens.push(full.slice(i, i + 4)); // ~4-char fragments
  const TOKEN_HZ = 150;
  const FPS = 60;
  const render = (text: string) => renderToString(React.createElement(ReactMarkdown, { remarkPlugins: [remarkGfm] }, text));
  const useTheme = BUILTIN_THEMES[0];
  const codeStart = full.indexOf('```ts\n') + 6;
  const run = (frameBatched: boolean) => {
    let acc = '';
    let lastFrame = -1;
    let hl = 0;
    let lastHl = -1e9;
    const t0 = now();
    tokens.forEach((tk, i) => {
      acc += tk;
      const tMs = (i / TOKEN_HZ) * 1000;
      if (frameBatched) {
        const frame = Math.floor((tMs / 1000) * FPS);
        if (frame === lastFrame && i !== tokens.length - 1) return;
        lastFrame = frame;
      }
      render(acc);
      // Lezer highlight of the open code block: every render (before) vs <= 1 per 120 ms (after)
      const open = acc.length > codeStart && !acc.slice(codeStart).includes('\n```');
      if (open && (!frameBatched || tMs - lastHl >= 120)) {
        highlightLinesByName(acc.slice(codeStart), 'ts', useTheme);
        lastHl = tMs;
        hl++;
      }
    });
    return { ms: now() - t0, hl };
  };
  render('warm');
  run(false); // JIT warm-up
  const b = run(false);
  const a = run(true);
  add(`Agent streaming CPU: markdown + highlight (${tokens.length} tokens @${TOKEN_HZ}/s; highlights ${b.hl} -> ${a.hl})`, b.ms, a.ms);
}

// ---- report ---------------------------------------------------------------------------------
const f = (n: number) => n.toFixed(n < 10 ? 2 : 1);
console.log('\n| Benchmark | Before (ms) | After (ms) | Speed-up | Saved |');
console.log('|---|---:|---:|---:|---:|');
for (const r of rows) {
  console.log(
    `| ${r.name} | ${f(r.before)} | ${f(r.after)} | ${(r.before / Math.max(r.after, 0.001)).toFixed(1)}x | ${(((r.before - r.after) / r.before) * 100).toFixed(0)}% |`,
  );
}
